#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use craft_jsonquery::query::{clip, from_params, run};
use craft_jsonquery::{Layout, Patch, QueryOpts};
use serde_json::{Value, json};

/// A limit no test answer comes near.
const ROOMY: usize = 1 << 20;

fn opts() -> QueryOpts {
    QueryOpts::new(ROOMY, Layout::Compact)
}

fn doc() -> Value {
    json!({
        "artboards": [{"name": "ace", "rect": {"x0": 0.0}}, {"name": "two", "rect": {"x0": 200.0}}],
        "layers": [{"id": 1, "kind": {"type": "layer", "children": [
            {"id": 2, "name": "paper", "kind": {"type": "path"}},
            {"id": 3, "name": "pip", "kind": {"type": "group", "children": [{"id": 4, "kind": {"type": "path"}}]}}
        ]}}]
    })
}

#[test]
fn matches_come_with_their_normalized_paths_and_pointers() {
    let v = run(&doc(), "$.artboards[*].name", opts()).unwrap();
    assert_eq!(v["total"], 2);
    assert_eq!(v["matches"][0], json!({"path": "$['artboards'][0]['name']", "pointer": "/artboards/0/name", "value": "ace"}));
    let v = run(&doc(), "$..[?@.kind.type == 'path'].id", opts()).unwrap();
    assert_eq!(v["matches"].as_array().unwrap().iter().map(|m| m["value"].as_u64().unwrap()).collect::<Vec<_>>(), [2, 4]);
}

#[test]
fn limit_offset_and_depth_bound_the_answer() {
    let v = run(&doc(), "$.layers[0].kind.children[*]", QueryOpts { limit: Some(1), offset: 1, depth: Some(1), ..opts() }).unwrap();
    assert_eq!(v["total"], 2);
    assert_eq!(v["matches"].as_array().unwrap().len(), 1);
    assert_eq!(v["matches"][0]["value"], json!({"id": 3, "name": "pip", "kind": "{…2 keys}"}));
    let v = run(&doc(), "$.layers[*]", QueryOpts { limit: Some(0), ..opts() }).unwrap();
    assert_eq!((v["total"].as_u64(), v["matches"].as_array().map(Vec::len)), (Some(1), Some(0)));
    assert_eq!(clip(&json!([[1, 2], 3]), Some(1)), json!(["[…2 items]", 3]));
}

#[test]
fn a_bad_path_and_an_oversized_answer_are_errors_that_say_what_to_do() {
    assert!(run(&doc(), "layers[", opts()).unwrap_err().contains("RFC 9535"));
    let big = Value::Array((0..3000).map(|i| json!({"i": i, "text": "x".repeat(400)})).collect());
    let err = run(&big, "$[*]", QueryOpts { limit: Some(3000), ..opts() }).unwrap_err();
    assert!(err.contains("narrow `path`"), "{err}");
}

#[test]
fn a_tools_arguments_are_read_and_checked() {
    let v = from_params(&doc(), &json!({"path": "$.artboards[*]", "limit": 1, "offset": 1, "depth": 0}), opts()).unwrap();
    assert_eq!(v["matches"], json!([{"path": "$['artboards'][1]", "pointer": "/artboards/1", "value": "{…2 keys}"}]));
    assert_eq!(from_params(&doc(), &json!({"path": "$.layers[*]", "limit": null}), opts()).unwrap()["total"], 1);
    assert!(from_params(&doc(), &json!({}), opts()).unwrap_err().contains("`path`"));
    assert!(from_params(&doc(), &json!({"path": "$", "limit": -1}), opts()).unwrap_err().contains("`limit` must be a whole number"));
    assert!(from_params(&doc(), &json!({"path": "$", "depth": "2"}), opts()).unwrap_err().contains("`depth`"));
}

#[test]
fn a_query_points_where_a_patch_writes() {
    let mut d = doc();
    let found = run(&d, "$..[?@.name == 'pip'].name", opts()).unwrap();
    let pointer = found["matches"][0]["pointer"].as_str().unwrap().to_string();
    let patch = Patch::parse(&json!([{"op": "replace", "path": pointer, "value": "pips"}])).unwrap();
    patch.apply(&mut d).unwrap();
    assert_eq!(d["layers"][0]["kind"]["children"][1]["name"], "pips");
}

/// Matches of every JSON kind, with escapes, multi-byte text and numbers serde writes long.
fn varied() -> Value {
    json!({"items": [
        {"name": "ace ♠", "tags": ["a\"b", "c\\d", "e\nf"], "at": [0.1, -2.5e-8, 1e300]},
        {"name": "two", "tags": [], "nested": {"deep": {"deeper": {"deepest": [1, [2, [3]]]}}}},
        {"name": "日本語の名前", "empty": {}, "none": null, "yes": true},
        {"name": "\u{1F600}", "n": u64::MAX, "neg": i64::MIN}
    ]})
}

fn written(v: &Value, layout: Layout) -> usize {
    match layout {
        Layout::Compact => serde_json::to_vec(v).unwrap().len(),
        Layout::Pretty => serde_json::to_vec_pretty(v).unwrap().len(),
    }
}

/// At every limit from 0 to past the whole answer, in both layouts: the answer is the whole
/// answer, written in no more than the limit, exactly when the whole answer fits; otherwise it
/// is refused. So the limit is never passed, and an answer that fits is never refused.
#[test]
fn the_byte_limit_is_kept_exactly_and_refuses_only_what_does_not_fit() {
    let doc = varied();
    let shapes: [(&str, Option<u64>, u64, Option<u64>); 6] = [
        ("$.items[*]", None, 0, None),
        ("$.items[*]", None, 0, Some(1)),
        ("$.items[*]", Some(2), 1, Some(2)),
        ("$..name", None, 0, None),
        ("$", None, 0, Some(3)),
        ("$.nothing", None, 0, None),
    ];
    for (path, limit, offset, depth) in shapes {
        for layout in [Layout::Compact, Layout::Pretty] {
            let shape = |max_bytes| QueryOpts { limit, offset, depth, max_bytes, layout };
            let whole = run(&doc, path, shape(usize::MAX)).unwrap();
            let need = written(&whole, layout);
            for cap in 0..=need + 8 {
                match run(&doc, path, shape(cap)) {
                    Ok(answer) => {
                        assert!(cap >= need, "{path} {layout:?}: answered at {cap} bytes, below the {need} it needs");
                        assert_eq!(answer, whole, "{path} {layout:?} at {cap}");
                        assert!(written(&answer, layout) <= cap, "{path} {layout:?} at {cap}");
                    }
                    Err(e) => assert!(cap < need, "{path} {layout:?}: refused at {cap} bytes, though the answer is {need}: {e}"),
                }
            }
        }
    }
}

#[test]
fn a_limit_too_small_for_any_answer_says_so() {
    let err = run(&doc(), "$.artboards", QueryOpts::new(10, Layout::Compact)).unwrap_err();
    assert!(err.contains("10 bytes is too few bytes for any answer"), "{err}");
    let err = run(&doc(), "$.artboards[*]", QueryOpts::new(64, Layout::Pretty)).unwrap_err();
    assert!(err.contains("more than 64 bytes pretty-printed"), "{err}");
    let err = run(&json!({"s": "x".repeat(2000)}), "$.s", QueryOpts::new(1 << 10, Layout::Compact)).unwrap_err();
    assert!(err.contains("more than 1 KiB, the most this query answers"), "{err}");
}

/// A tool's arguments set limit, offset and depth; the byte limit and the layout are the app's.
#[test]
fn a_tools_arguments_cannot_raise_the_byte_limit() {
    let app = QueryOpts::new(256, Layout::Pretty);
    let err = from_params(&varied(), &json!({"path": "$", "max_bytes": 1_000_000_000_u64, "layout": "compact"}), app).unwrap_err();
    assert!(err.contains("more than 256 bytes pretty-printed"), "{err}");
}

/// One match far past the limit is refused, and measuring it stops at the limit rather than
/// writing the whole of it.
#[test]
fn a_huge_match_is_refused_at_the_limit() {
    let doc = json!({"blob": "x".repeat(32 << 20)});
    let err = run(&doc, "$.blob", QueryOpts::new(1 << 10, Layout::Compact)).unwrap_err();
    assert!(err.contains("narrow `path`"), "{err}");
    assert_eq!(run(&doc, "$.blob", QueryOpts { limit: Some(0), ..QueryOpts::new(1 << 10, Layout::Compact) }).unwrap()["total"], 1);
}
