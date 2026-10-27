#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use craft_jsonquery::Patch;
use serde_json::json;

#[test]
fn a_patch_applies_in_order_all_or_none() {
    let mut doc = json!({"artboards": [{"name": "a"}], "layers": []});
    let patch = Patch::parse(
        &json!([{"op": "replace", "path": "/artboards/0/name", "value": "ace"}, {"op": "add", "path": "/artboards/-", "value": {"name": "two"}}]),
    )
    .unwrap();
    assert_eq!(patch.len(), 2);
    assert_eq!(patch.apply(&mut doc).unwrap(), 2);
    assert_eq!(doc["artboards"], json!([{"name": "ace"}, {"name": "two"}]));
    let before = doc.clone();
    let failing = Patch::parse(
        &json!([{"op": "replace", "path": "/artboards/0/name", "value": "x"}, {"op": "test", "path": "/artboards/1/name", "value": "nope"}]),
    )
    .unwrap();
    let err = failing.apply(&mut doc).unwrap_err();
    assert!(err.contains("'/1'") && err.contains("/artboards/1/name"), "{err}");
    assert_eq!(doc, before, "nothing of a failed patch is applied");
}

#[test]
fn a_patch_that_is_not_one_is_refused_with_what_one_is() {
    for bad in [json!(null), json!([]), json!({"op": "add"}), json!("replace")] {
        let err = Patch::parse(&bad).unwrap_err();
        assert!(err.contains("non-empty array of JSON Patch operations"), "{bad}: {err}");
    }
    let err = Patch::parse(&json!([{"op": "rename", "path": "/a"}])).unwrap_err();
    assert!(err.contains("not a JSON Patch (RFC 6902)"), "{err}");
    let err = Patch::parse(&json!([{"op": "add", "path": "/a"}])).unwrap_err();
    assert!(err.contains("not a JSON Patch"), "an add without a value: {err}");
}

#[test]
fn a_tools_arguments_carry_the_patch() {
    let patch = Patch::from_params(&json!({"patch": [{"op": "remove", "path": "/a"}], "label": "Tidy"})).unwrap();
    let mut doc = json!({"a": 1, "b": 2});
    patch.apply(&mut doc).unwrap();
    assert_eq!(doc, json!({"b": 2}));
    assert!(Patch::from_params(&json!({"label": "Tidy"})).is_err());
}
