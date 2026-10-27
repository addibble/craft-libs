//! JSONPath ([RFC 9535](https://www.rfc-editor.org/rfc/rfc9535)) over a model's JSON, so that an
//! agent can look at one part of a large document (`$.artboards[*].name`,
//! `$.layers[0].kind.children[?@.kind.type == 'path']`, `$..[?@.name == 'logo']`) without reading
//! the whole model. The answer is bounded: at most `limit` matches (`total` counts them all), each
//! match's value cut at `depth` levels with what lies below summarized, and never more than
//! [`MAX_BYTES`] of JSON.

use serde_json::{Value, json};
use serde_json_path::JsonPath;

/// Most matches listed unless `limit` says otherwise.
pub const DEFAULT_LIMIT: u64 = 100;
/// Most bytes of JSON an answer carries: past it, the query must be narrowed.
pub const MAX_BYTES: usize = 1 << 20;

/// How much of the matches to answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueryOpts {
    /// At most this many matches listed (`None`: [`DEFAULT_LIMIT`]); 0 counts only.
    pub limit: Option<u64>,
    /// Matches skipped before the first listed.
    pub offset: u64,
    /// Levels of each match's value shown; containers below are summarized (`None`: all).
    pub depth: Option<u64>,
}

/// The matches of `path` in `value`: `{matches: [{path, pointer, value}], total}`, where `path`
/// is the match's normalized path (`$['layers'][0]`), so a later query names it exactly, and
/// `pointer` the same as a JSON Pointer (`/layers/0`), what a [`crate::Patch`] takes.
pub fn run(value: &Value, path: &str, opts: QueryOpts) -> Result<Value, String> {
    let path = JsonPath::parse(path).map_err(|e| format!("`path` is not a JSONPath (RFC 9535): {e}"))?;
    let located = path.query_located(value);
    let total = located.len();
    let limit = usize::try_from(opts.limit.unwrap_or(DEFAULT_LIMIT)).unwrap_or(usize::MAX);
    let offset = usize::try_from(opts.offset).unwrap_or(usize::MAX);
    let matches: Vec<Value> = located
        .iter()
        .skip(offset)
        .take(limit)
        .map(|m| json!({"path": m.location().to_string(), "pointer": m.location().to_json_pointer(), "value": clip(m.node(), opts.depth)}))
        .collect();
    let answer = json!({"matches": matches, "total": total});
    let bytes = serde_json::to_string(&answer).map_or(0, |s| s.len());
    if bytes > MAX_BYTES {
        return Err(format!("the matches come to {bytes} bytes, past the {MAX_BYTES} a query answers: narrow `path`, lower `limit` or set `depth`"));
    }
    Ok(answer)
}

/// [`run`] with a tool's arguments, `{path, limit?, offset?, depth?}` (the `query_document` tool's
/// input, [`crate::tools::query_document`]).
pub fn from_params(value: &Value, params: &Value) -> Result<Value, String> {
    let path = params.get("path").and_then(Value::as_str).ok_or("`path` must be a JSONPath string, such as $.items[*].name")?;
    let opts = QueryOpts { limit: count(params, "limit")?, offset: count(params, "offset")?.unwrap_or(0), depth: count(params, "depth")? };
    run(value, path, opts)
}

/// The whole number at `key` of `params`, or none when it is left out or null.
fn count(params: &Value, key: &str) -> Result<Option<u64>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| format!("`{key}` must be a whole number, 0 or more")),
    }
}

/// `v` cut at `depth` levels: a container below that is replaced by a summary of what it holds
/// (`{…12 keys}`, `[…54 items]`), never silently dropped.
pub fn clip(v: &Value, depth: Option<u64>) -> Value {
    match (v, depth) {
        (Value::Object(o), Some(0)) => Value::String(format!("{{…{} keys}}", o.len())),
        (Value::Array(a), Some(0)) => Value::String(format!("[…{} items]", a.len())),
        (Value::Object(o), d) => Value::Object(o.iter().map(|(k, v)| (k.clone(), clip(v, d.map(|d| d - 1)))).collect()),
        (Value::Array(a), d) => Value::Array(a.iter().map(|v| clip(v, d.map(|d| d - 1))).collect()),
        (v, _) => v.clone(),
    }
}
