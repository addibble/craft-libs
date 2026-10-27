//! JSONPath ([RFC 9535](https://www.rfc-editor.org/rfc/rfc9535)) over a model's JSON, so that an
//! agent can look at one part of a large document (`$.artboards[*].name`,
//! `$.layers[0].kind.children[?@.kind.type == 'path']`, `$..[?@.name == 'logo']`) without reading
//! the whole model. The answer is bounded: at most `limit` matches (`total` counts them all), each
//! match's value cut at `depth` levels with what lies below summarized, and never more than the
//! app's `max_bytes` of JSON as the app writes it ([`Layout`]).
//!
//! The byte limit is kept exactly. Each match is measured as it would be written, before any of
//! the answer is built, and measuring stops at the first byte past what is left, so an answer
//! that does not fit costs no more work than the limit allows. The answer built is then written
//! once more in the app's layout, again stopping at the limit, and refused if it is past it.

use std::io;

use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::{Value, json};
use serde_json_path::{JsonPath, LocatedNode};

/// Most matches listed unless `limit` says otherwise.
pub const DEFAULT_LIMIT: u64 = 100;

/// How the app writes an answer, which is what its byte limit counts: the answer as `serde_json`
/// writes it in this layout, the text the agent reads. An envelope the app puts it in (an MCP
/// result, which carries the text as an escaped JSON string) is not counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// No whitespace (`serde_json::to_vec`, `to_string`).
    Compact,
    /// Indented by two spaces (`serde_json::to_vec_pretty`, `to_string_pretty`).
    Pretty,
}

/// How much of the matches to answer. `max_bytes` and `layout` are the app's: there is no
/// default, and a tool's arguments never change them ([`from_params`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryOpts {
    /// At most this many matches listed (`None`: [`DEFAULT_LIMIT`]); 0 counts only.
    pub limit: Option<u64>,
    /// Matches skipped before the first listed.
    pub offset: u64,
    /// Levels of each match's value shown; containers below are summarized (`None`: all).
    pub depth: Option<u64>,
    /// Most bytes the answer comes to, written in `layout`.
    pub max_bytes: usize,
    /// How the app writes the answer.
    pub layout: Layout,
}

impl QueryOpts {
    /// Every match up to [`DEFAULT_LIMIT`], whole, in at most `max_bytes` written in `layout`.
    pub fn new(max_bytes: usize, layout: Layout) -> QueryOpts {
        QueryOpts { limit: None, offset: 0, depth: None, max_bytes, layout }
    }
}

/// The matches of `path` in `value`: `{matches: [{path, pointer, value}], total}`, where `path`
/// is the match's normalized path (`$['layers'][0]`), so a later query names it exactly, and
/// `pointer` the same as a JSON Pointer (`/layers/0`), what a [`crate::Patch`] takes. Written in
/// `opts.layout`, the answer is never more than `opts.max_bytes`; one that would be is refused
/// with what to narrow.
pub fn run(value: &Value, path: &str, opts: QueryOpts) -> Result<Value, String> {
    let path = JsonPath::parse(path).map_err(|e| format!("`path` is not a JSONPath (RFC 9535): {e}"))?;
    let located = path.query_located(value);
    let total = located.len();
    let limit = usize::try_from(opts.limit.unwrap_or(DEFAULT_LIMIT)).unwrap_or(usize::MAX);
    let offset = usize::try_from(opts.offset).unwrap_or(usize::MAX);
    let listed: Vec<LocatedNode<'_>> = located.into_iter().skip(offset).take(limit).collect();

    // What the answer comes to with no match listed, then each match with the comma before it:
    // compact JSON is the sum of its parts, and no layout writes fewer bytes, so a compact sum
    // past the limit is past it in every layout. Measuring stops at the limit.
    let empty = json!({"matches": [], "total": total});
    let mut left = opts
        .max_bytes
        .checked_sub(written_len(&empty))
        .ok_or_else(|| format!("{} is too few bytes for any answer: the app's limit is too small", size(opts.max_bytes)))?;
    for (i, m) in listed.iter().enumerate() {
        let comma = usize::from(i > 0);
        left = left.checked_sub(comma).ok_or_else(|| past(opts))?;
        let entry = Entry { path: &m.location().to_string(), pointer: &m.location().to_json_pointer(), value: Clipped(m.node(), opts.depth) };
        left -= measure(&entry, left, Layout::Compact).ok_or_else(|| past(opts))?;
    }

    let matches: Vec<Value> = listed
        .iter()
        .map(|m| json!({"path": m.location().to_string(), "pointer": m.location().to_json_pointer(), "value": clip(m.node(), opts.depth)}))
        .collect();
    let answer = json!({"matches": matches, "total": total});
    // The answer as the app writes it, checked byte for byte: a pretty answer is longer than the
    // compact measure, and this is what keeps the limit whatever the layout.
    measure(&answer, opts.max_bytes, opts.layout).ok_or_else(|| past(opts))?;
    Ok(answer)
}

/// [`run`] with a tool's arguments, `{path, limit?, offset?, depth?}` (the `query_document` tool's
/// input, [`crate::tools::query_document`]), on the app's `opts`: the arguments set `limit`,
/// `offset` and `depth`, never `max_bytes` or `layout`.
pub fn from_params(value: &Value, params: &Value, opts: QueryOpts) -> Result<Value, String> {
    let path = params.get("path").and_then(Value::as_str).ok_or("`path` must be a JSONPath string, such as $.items[*].name")?;
    let opts = QueryOpts { limit: count(params, "limit")?, offset: count(params, "offset")?.unwrap_or(0), depth: count(params, "depth")?, ..opts };
    run(value, path, opts)
}

/// The whole number at `key` of `params`, or none when it is left out or null.
fn count(params: &Value, key: &str) -> Result<Option<u64>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| format!("`{key}` must be a whole number, 0 or more")),
    }
}

/// Why an answer is refused when its matches don't fit.
fn past(opts: QueryOpts) -> String {
    let layout = match opts.layout {
        Layout::Compact => "",
        Layout::Pretty => " pretty-printed",
    };
    format!(
        "the matches come to more than {}{layout}, the most this query answers: narrow `path`, lower `limit` or set `depth`",
        size(opts.max_bytes)
    )
}

/// `bytes` as a person reads it: `1 MiB`, `64 KiB`, `1000 bytes`.
pub(crate) fn size(bytes: usize) -> String {
    const KIB: usize = 1 << 10;
    const MIB: usize = 1 << 20;
    match bytes {
        0 => "0 bytes".into(),
        b if b % MIB == 0 => format!("{} MiB", b / MIB),
        b if b % KIB == 0 => format!("{} KiB", b / KIB),
        b => format!("{b} bytes"),
    }
}

/// The bytes `v` comes to as compact JSON (small values only: it is written in full).
fn written_len(v: &Value) -> usize {
    let mut counter = Budget { used: 0, cap: usize::MAX };
    match serde_json::to_writer(&mut counter, v) {
        Ok(()) => counter.used,
        Err(_) => usize::MAX,
    }
}

/// The bytes `v` comes to written in `layout`, or none when that is past `cap`: writing stops at
/// the first byte past it.
fn measure(v: &impl Serialize, cap: usize, layout: Layout) -> Option<usize> {
    let mut budget = Budget { used: 0, cap };
    let written = match layout {
        Layout::Compact => serde_json::to_writer(&mut budget, v),
        Layout::Pretty => serde_json::to_writer_pretty(&mut budget, v),
    };
    // Writing JSON from a value fails only when the writer refuses: past the cap.
    written.ok().map(|()| budget.used)
}

/// A writer that keeps nothing and counts, refusing the write that would take it past `cap`.
struct Budget {
    used: usize,
    cap: usize,
}

impl io::Write for Budget {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let used = self.used.checked_add(buf.len()).filter(|&u| u <= self.cap).ok_or_else(|| io::Error::other("past the byte limit"))?;
        self.used = used;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// One match as the answer lists it, written without building it: `{path, pointer, value}`, the
/// keys in the order a `serde_json` object keeps them (sorted, or as inserted with
/// `preserve_order`; the same here).
struct Entry<'a> {
    path: &'a str,
    pointer: &'a str,
    value: Clipped<'a>,
}

impl Serialize for Entry<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(3))?;
        map.serialize_entry("path", self.path)?;
        map.serialize_entry("pointer", self.pointer)?;
        map.serialize_entry("value", &self.value)?;
        map.end()
    }
}

/// [`clip`] of a value, written as it is read instead of copied: the same JSON, byte for byte.
struct Clipped<'a>(&'a Value, Option<u64>);

impl Serialize for Clipped<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match (self.0, self.1) {
            (Value::Object(o), Some(0)) => s.serialize_str(&keys_summary(o.len())),
            (Value::Array(a), Some(0)) => s.serialize_str(&items_summary(a.len())),
            (Value::Object(o), d) => {
                let mut map = s.serialize_map(Some(o.len()))?;
                for (k, v) in o {
                    map.serialize_entry(k, &Clipped(v, d.map(|d| d - 1)))?;
                }
                map.end()
            }
            (Value::Array(a), d) => {
                let mut seq = s.serialize_seq(Some(a.len()))?;
                for v in a {
                    seq.serialize_element(&Clipped(v, d.map(|d| d - 1)))?;
                }
                seq.end()
            }
            (v, _) => v.serialize(s),
        }
    }
}

fn keys_summary(n: usize) -> String {
    format!("{{…{n} keys}}")
}

fn items_summary(n: usize) -> String {
    format!("[…{n} items]")
}

/// `v` cut at `depth` levels: a container below that is replaced by a summary of what it holds
/// (`{…12 keys}`, `[…54 items]`), never silently dropped.
pub fn clip(v: &Value, depth: Option<u64>) -> Value {
    match (v, depth) {
        (Value::Object(o), Some(0)) => Value::String(keys_summary(o.len())),
        (Value::Array(a), Some(0)) => Value::String(items_summary(a.len())),
        (Value::Object(o), d) => Value::Object(o.iter().map(|(k, v)| (k.clone(), clip(v, d.map(|d| d - 1)))).collect()),
        (Value::Array(a), d) => Value::Array(a.iter().map(|v| clip(v, d.map(|d| d - 1))).collect()),
        (v, _) => v.clone(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{Clipped, Entry, Layout, clip, measure};
    use serde_json::json;

    /// Written, a [`Clipped`] value and an [`Entry`] are byte for byte the values the answer is
    /// built from, in both layouts, which is what lets measuring stand in for building.
    #[test]
    fn measuring_writes_what_building_would() {
        let doc = json!({"b": [1, 2.5, -3e-7, {"x": "a\"\\\n\u{1F600}…"}], "a": {"z": null, "y": [true, []], "x": {}}, "c": ""});
        for depth in [None, Some(0), Some(1), Some(2), Some(3), Some(9)] {
            for layout in [Layout::Compact, Layout::Pretty] {
                let built = clip(&doc, depth);
                assert_eq!(measure(&Clipped(&doc, depth), usize::MAX, layout), measure(&built, usize::MAX, layout), "{depth:?} {layout:?}");
                let entry = Entry { path: "$['b'][3]", pointer: "/b/3", value: Clipped(&doc, depth) };
                let built = json!({"path": "$['b'][3]", "pointer": "/b/3", "value": built});
                let (a, b) = (serde_json::to_vec(&entry).unwrap(), serde_json::to_vec(&built).unwrap());
                assert_eq!(a, b, "{depth:?}");
            }
        }
    }

    #[test]
    fn measuring_stops_at_the_cap() {
        let v = json!(["abcdef", "ghijkl"]);
        let full = measure(&v, usize::MAX, Layout::Compact).unwrap();
        assert_eq!(measure(&v, full, Layout::Compact), Some(full));
        assert_eq!(measure(&v, full - 1, Layout::Compact), None);
        assert_eq!(measure(&v, 0, Layout::Compact), None);
        let pretty = measure(&v, usize::MAX, Layout::Pretty).unwrap();
        assert!(pretty > full);
        assert_eq!(measure(&v, pretty - 1, Layout::Pretty), None);
    }
}
