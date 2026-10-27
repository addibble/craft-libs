#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use craft_jsonquery::query::{clip, from_params, run};
use craft_jsonquery::{Patch, QueryOpts};
use serde_json::{Value, json};

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
    let v = run(&doc(), "$.artboards[*].name", QueryOpts::default()).unwrap();
    assert_eq!(v["total"], 2);
    assert_eq!(v["matches"][0], json!({"path": "$['artboards'][0]['name']", "pointer": "/artboards/0/name", "value": "ace"}));
    let v = run(&doc(), "$..[?@.kind.type == 'path'].id", QueryOpts::default()).unwrap();
    assert_eq!(v["matches"].as_array().unwrap().iter().map(|m| m["value"].as_u64().unwrap()).collect::<Vec<_>>(), [2, 4]);
}

#[test]
fn limit_offset_and_depth_bound_the_answer() {
    let v = run(&doc(), "$.layers[0].kind.children[*]", QueryOpts { limit: Some(1), offset: 1, depth: Some(1) }).unwrap();
    assert_eq!(v["total"], 2);
    assert_eq!(v["matches"].as_array().unwrap().len(), 1);
    assert_eq!(v["matches"][0]["value"], json!({"id": 3, "name": "pip", "kind": "{…2 keys}"}));
    let v = run(&doc(), "$.layers[*]", QueryOpts { limit: Some(0), ..QueryOpts::default() }).unwrap();
    assert_eq!((v["total"].as_u64(), v["matches"].as_array().map(Vec::len)), (Some(1), Some(0)));
    assert_eq!(clip(&json!([[1, 2], 3]), Some(1)), json!(["[…2 items]", 3]));
}

#[test]
fn a_bad_path_and_an_oversized_answer_are_errors_that_say_what_to_do() {
    assert!(run(&doc(), "layers[", QueryOpts::default()).unwrap_err().contains("RFC 9535"));
    let big = Value::Array((0..3000).map(|i| json!({"i": i, "text": "x".repeat(400)})).collect());
    let err = run(&big, "$[*]", QueryOpts { limit: Some(3000), ..QueryOpts::default() }).unwrap_err();
    assert!(err.contains("narrow `path`"), "{err}");
}

#[test]
fn a_tools_arguments_are_read_and_checked() {
    let v = from_params(&doc(), &json!({"path": "$.artboards[*]", "limit": 1, "offset": 1, "depth": 0})).unwrap();
    assert_eq!(v["matches"], json!([{"path": "$['artboards'][1]", "pointer": "/artboards/1", "value": "{…2 keys}"}]));
    assert_eq!(from_params(&doc(), &json!({"path": "$.layers[*]", "limit": null})).unwrap()["total"], 1);
    assert!(from_params(&doc(), &json!({})).unwrap_err().contains("`path`"));
    assert!(from_params(&doc(), &json!({"path": "$", "limit": -1})).unwrap_err().contains("`limit` must be a whole number"));
    assert!(from_params(&doc(), &json!({"path": "$", "depth": "2"})).unwrap_err().contains("`depth`"));
}

#[test]
fn a_query_points_where_a_patch_writes() {
    let mut d = doc();
    let found = run(&d, "$..[?@.name == 'pip'].name", QueryOpts::default()).unwrap();
    let pointer = found["matches"][0]["pointer"].as_str().unwrap().to_string();
    let patch = Patch::parse(&json!([{"op": "replace", "path": pointer, "value": "pips"}])).unwrap();
    patch.apply(&mut d).unwrap();
    assert_eq!(d["layers"][0]["kind"]["children"][1]["name"], "pips");
}
