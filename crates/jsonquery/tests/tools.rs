#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use craft_jsonquery::tools::{all, patch_document, query_document};
use craft_jsonquery::{ModelText, Patch, Schema};
use serde_json::{Value, json};

fn text() -> ModelText {
    ModelText {
        model: "document model".into(),
        source: "as document.json gives it".into(),
        query_examples: vec!["$.artboards[*].name".into(), "$..[?@.name == 'logo']".into()],
        root: "Document".into(),
        type_examples: vec!["Artboard".into(), "Node".into()],
        pointer_example: "/layers/0/kind/children/2/opacity".into(),
        patch_note: "An object added without an id gets a fresh one.".into(),
    }
}

#[test]
fn the_three_tools_are_listed_in_the_order_an_agent_uses_them() {
    let tools = all(&text());
    let names: Vec<&str> = tools.iter().map(|t| t.name).collect();
    assert_eq!(names, ["query_document", "document_schema", "patch_document"]);
    let read_only: Vec<bool> = tools.iter().map(|t| t.read_only).collect();
    assert_eq!(read_only, [true, true, false]);
    for t in &tools {
        let listed = t.to_json();
        assert_eq!(listed["name"], t.name);
        assert_eq!(listed["inputSchema"]["type"], "object");
        assert_eq!(listed["inputSchema"]["additionalProperties"], false, "{}: unknown keys are refused", t.name);
        assert_eq!(listed["annotations"]["readOnlyHint"], t.read_only);
        assert!(!t.description.contains("{model}") && !t.description.contains("  "), "{}: {}", t.name, t.description);
    }
}

#[test]
fn the_descriptions_speak_of_the_apps_model() {
    let q = query_document(&text());
    assert!(
        q.description.contains("over the document model as document.json gives it, for one part of a large document: $.artboards[*].name"),
        "{}",
        q.description
    );
    assert!(q.description.contains("never past 1 MiB"), "{}", q.description);
    let p = patch_document(&text());
    assert!(p.description.ends_with("One undo step, named `label`. An object added without an id gets a fresh one."), "{}", p.description);
    assert!(p.input_schema["properties"]["patch"]["items"]["description"].as_str().unwrap().contains("/layers/0/kind/children/2/opacity"));
    let bare = ModelText { source: String::new(), query_examples: vec![], patch_note: String::new(), ..text() };
    assert!(query_document(&bare).description.contains("over the document model, for one part of a large document. Answers"));
    assert!(patch_document(&bare).description.ends_with("named `label`."));
}

/// Every argument a tool's schema names is one its handler reads, and its required ones are.
#[test]
fn each_tools_arguments_are_the_ones_its_handler_reads() {
    let props = |t: &craft_jsonquery::ToolDef| -> Vec<String> { t.input_schema["properties"].as_object().unwrap().keys().cloned().collect() };
    let [q, s, p] = <[_; 3]>::try_from(all(&text())).unwrap();
    assert_eq!(props(&q), ["depth", "limit", "offset", "path"]);
    assert_eq!(q.input_schema["required"], json!(["path"]));
    let doc = json!({"a": [1, 2]});
    assert!(craft_jsonquery::query::from_params(&doc, &json!({"path": "$.a[*]", "limit": 1, "offset": 0, "depth": 0})).is_ok());
    assert_eq!(props(&s), ["at", "index", "type"]);
    let schema = Schema::new(json!({"type": "object", "properties": {"a": {"type": "array", "items": {"type": "integer"}}}}), "Model");
    assert_eq!(schema.answer(&json!({"at": "$.a[0]"}), || Ok::<Value, String>(doc.clone())).unwrap()["shape"], "integer");
    assert_eq!(props(&p), ["label", "patch"]);
    assert!(Patch::from_params(&json!({"patch": [{"op": "remove", "path": "/a/0"}], "label": "Tidy"})).is_ok());
}
