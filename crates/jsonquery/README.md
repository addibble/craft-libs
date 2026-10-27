# Craft JSON Query

`craft-jsonquery` lets an agent read and change a large JSON model a part at a time, by standards
it already knows, so every Craft app's MCP server can offer the same three tools over its own
document model:

- `query_document`: JSONPath ([RFC 9535](https://www.rfc-editor.org/rfc/rfc9535)) over the model.
  Each match comes with its normalized path and its JSON Pointer, and the answer is bounded by
  `limit`, `offset`, `depth` (what lies below is summarized, never silently dropped) and a 1 MiB
  ceiling that says what to narrow.
- `document_schema`: the model's JSON Schema, read one type at a time: the root, one named type
  (its properties' types, defaults and descriptions, and the types to read next), the type of the
  value at a JSONPath into the live document (an enum resolved to the variant the value is), or an
  index of every type.
- `patch_document`: JSON Patch ([RFC 6902](https://www.rfc-editor.org/rfc/rfc6902)) at the JSON
  Pointers ([RFC 6901](https://www.rfc-editor.org/rfc/rfc6901)) a query answers, applied in
  order, all or none.

## Boundaries

- The app keeps its model: it serializes the model to a `serde_json::Value`, loads a patched value
  back through the model (which refuses what the model cannot be), fixes up what its model needs
  first (fresh ids for objects added without one, say), and records the change as one undo step.
- The schema is the app's: it generates it from its own types and doc comments with `schemars`
  (0.8 or 1.x; both are read) and hands it over as JSON, so it never lags the format and this crate
  does not depend on `schemars`.
- Never crashes: no panics, no `unsafe`. A bad path, patch or argument is an error that says what
  to do instead.
- No I/O; compiles for `wasm32-unknown-unknown`. Dependencies: `serde_json`, `serde_json_path` and
  `json-patch`.

## Usage

```rust
use craft_jsonquery::tools::{self, ModelText};
use craft_jsonquery::{Patch, Schema, query};
use serde_json::json;

// The app's model as JSON, and its schema (from `schemars::schema_for!(Document)` in an app).
let mut doc = json!({"artboards": [{"name": "ace"}, {"name": "two"}]});
let schema = Schema::new(
    json!({
        "type": "object",
        "properties": {"artboards": {"type": "array", "items": {"$ref": "#/$defs/Artboard"}}},
        "$defs": {"Artboard": {"description": "One page.", "type": "object", "properties": {"name": {"type": "string"}}}}
    }),
    "Document",
);

// The tools, for `tools/list`.
let text = ModelText {
    model: "document model".into(),
    source: String::new(),
    query_examples: vec!["$.artboards[*].name".into()],
    root: schema.root().into(),
    type_examples: vec!["Artboard".into()],
    pointer_example: "/artboards/0/name".into(),
    patch_note: String::new(),
};
let listed: Vec<_> = tools::all(&text).iter().map(|t| t.to_json()).collect();
assert_eq!(listed.len(), 3);

// `query_document`: the matches, each with the pointer a patch takes.
let found = query::from_params(&doc, &json!({"path": "$.artboards[?@.name == 'two']"})).unwrap();
assert_eq!(found["matches"][0]["pointer"], "/artboards/1");

// `document_schema`: the type there.
let at = schema.answer(&json!({"at": "$.artboards[1]"}), || Ok(doc.clone())).unwrap();
assert_eq!(at["type"], "Artboard");

// `patch_document`: all or none; the app then loads `doc` through its model as one undo step.
let patch = Patch::from_params(&json!({"patch": [{"op": "replace", "path": "/artboards/1/name", "value": "deuce"}]})).unwrap();
patch.apply(&mut doc).unwrap();
assert_eq!(doc["artboards"][1]["name"], "deuce");
```
