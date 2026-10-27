#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The same model's schema from schemars 0.8 and from 1.x reads the same way.

use craft_jsonquery::Schema;
use serde_json::{Value, json};

/// A small document model, its schema derived by the schemars crate `$krate`.
macro_rules! model {
    ($krate:literal) => {
        use std::collections::BTreeMap;

        use serde::Serialize;

        /// A drawing: its artboards and the art on them.
        #[derive(Serialize, JsonSchema)]
        #[schemars(crate = $krate)]
        pub struct Document {
            /// The document's title.
            pub title: String,
            pub artboards: Vec<Artboard>,
            /// The art, bottom first.
            pub layers: Vec<Node>,
            /// Colours by name.
            pub swatches: BTreeMap<String, Color>,
            /// Where the rulers measure from, when moved.
            pub origin: Option<[f64; 2]>,
        }

        /// One page of the drawing.
        #[derive(Serialize, JsonSchema)]
        #[schemars(crate = $krate)]
        pub struct Artboard {
            pub name: String,
            /// Its bounds, in points.
            pub rect: Rect,
            #[serde(default)]
            pub align: Align,
        }

        #[derive(Serialize, JsonSchema)]
        #[schemars(crate = $krate)]
        pub struct Rect {
            pub x0: f64,
            pub y0: f64,
            pub x1: f64,
            pub y1: f64,
        }

        /// Where an artboard's name sits.
        #[allow(dead_code)]
        #[derive(Serialize, JsonSchema, Default)]
        #[schemars(crate = $krate)]
        #[serde(rename_all = "camelCase")]
        pub enum Align {
            #[default]
            Left,
            Center,
            Right,
        }

        #[derive(Serialize, JsonSchema)]
        #[schemars(crate = $krate)]
        pub struct Color {
            pub rgb: [f64; 3],
        }

        /// One object in the tree.
        #[derive(Serialize, JsonSchema)]
        #[schemars(crate = $krate)]
        pub struct Node {
            pub id: u64,
            pub name: Option<String>,
            pub kind: Kind,
        }

        /// What an object is.
        #[derive(Serialize, JsonSchema)]
        #[schemars(crate = $krate)]
        #[serde(tag = "type", rename_all = "camelCase")]
        pub enum Kind {
            /// A layer of the document.
            Layer { children: Vec<Node> },
            /// A path.
            Path {
                /// Its geometry, as SVG path data.
                d: String,
            },
        }

        pub fn sample() -> Document {
            Document {
                title: "Deck".into(),
                artboards: vec![Artboard { name: "ace".into(), rect: Rect { x0: 0.0, y0: 0.0, x1: 180.0, y1: 252.0 }, align: Align::Center }],
                layers: vec![Node {
                    id: 1,
                    name: None,
                    kind: Kind::Layer { children: vec![Node { id: 2, name: Some("pip".into()), kind: Kind::Path { d: "M0 0L10 0".into() } }] },
                }],
                swatches: BTreeMap::from([("Red".to_string(), Color { rgb: [1.0, 0.0, 0.0] })]),
                origin: Some([10.0, 20.0]),
            }
        }
    };
}

mod v08 {
    use schemars08::JsonSchema;
    model!("schemars08");
}

mod v1 {
    use schemars1::JsonSchema;
    model!("schemars1");
}

/// The model's schema and a document, from each schemars line.
fn both() -> Vec<(&'static str, Schema, Value)> {
    vec![
        (
            "0.8",
            Schema::new(serde_json::to_value(schemars08::schema_for!(v08::Document)).unwrap(), "Document"),
            serde_json::to_value(v08::sample()).unwrap(),
        ),
        (
            "1.x",
            Schema::new(serde_json::to_value(schemars1::schema_for!(v1::Document)).unwrap(), "Document"),
            serde_json::to_value(v1::sample()).unwrap(),
        ),
    ]
}

fn property<'a>(rendered: &'a Value, name: &str) -> &'a Value {
    rendered["properties"].as_array().unwrap().iter().find(|p| p["name"] == name).unwrap_or_else(|| panic!("{name} in {rendered}"))
}

#[test]
fn the_root_describes_the_document_and_names_the_types_to_read_next() {
    for (line, schema, _) in both() {
        let root = schema.describe(schema.root()).unwrap();
        assert_eq!(root["type"], "Document", "{line}");
        assert!(root["description"].as_str().unwrap().starts_with("A drawing"), "{line}: {root}");
        assert_eq!(property(&root, "artboards")["type"], "array of Artboard", "{line}");
        assert_eq!(property(&root, "layers")["type"], "array of Node", "{line}");
        assert_eq!(property(&root, "swatches")["type"], "map of Color", "{line}");
        assert_eq!(property(&root, "origin")["type"], "[number, number] | null", "{line}: {root}");
        assert_eq!(property(&root, "layers")["description"], "The art, bottom first.", "{line}");
        assert_eq!(property(&root, "title")["required"], true, "{line}");
        assert_eq!(root["types"], json!(["Artboard", "Color", "Node"]), "{line}");
        let artboard = schema.describe("Artboard").unwrap();
        assert_eq!(property(&artboard, "rect")["type"], "Rect", "{line}: {artboard}");
        assert!(property(&artboard, "rect")["description"].as_str().unwrap().contains("points"), "{line}: {artboard}");
        assert_eq!(property(&artboard, "align")["default"], "left", "{line}: {artboard}");
        assert_eq!(schema.describe("Align").unwrap()["values"], json!(["left", "center", "right"]), "{line}");
    }
}

#[test]
fn a_tagged_enum_lists_its_variants_by_tag() {
    for (line, schema, _) in both() {
        let kind = schema.describe("Kind").unwrap();
        let variants = kind["variants"].as_array().unwrap();
        let tags: Vec<&str> = variants.iter().map(|v| v["tag"].as_str().unwrap()).collect();
        assert_eq!(tags, ["layer", "path"], "{line}: {kind}");
        let path = &variants[1];
        assert_eq!(path["description"], "A path.", "{line}");
        assert_eq!(property(path, "d")["type"], "string", "{line}");
        assert!(path["properties"].as_array().unwrap().iter().all(|p| p["name"] != "type"), "{line}: the tag is not a property");
    }
}

#[test]
fn the_type_at_a_path_follows_the_value_into_an_enum_variant() {
    for (line, schema, doc) in both() {
        let kind = schema.at(&doc, "$.layers[0].kind").unwrap();
        assert_eq!(kind["path"], "$['layers'][0]['kind']", "{line}");
        assert_eq!(kind["valueType"], "object", "{line}");
        assert_eq!(kind["tag"], "layer", "{line}: {kind}");
        assert_eq!(property(&kind, "children")["type"], "array of Node", "{line}: {kind}");
        let leaf = schema.at(&doc, "$.layers[0].kind.children[0].kind").unwrap();
        assert_eq!(leaf["tag"], "path", "{line}: {leaf}");
        assert_eq!(schema.at(&doc, "$.artboards[0].rect").unwrap()["type"], "Rect", "{line}");
        assert_eq!(schema.at(&doc, "$.swatches.Red").unwrap()["type"], "Color", "{line}");
        assert_eq!(schema.at(&doc, "$.layers[*].id").unwrap()["matches"], 1, "{line}");
        assert_eq!(schema.at(&doc, "$.origin[1]").unwrap()["valueType"], "number", "{line}");
        assert!(schema.at(&doc, "$.artboards[9]").unwrap_err().contains("nothing"), "{line}");
        assert!(schema.at(&doc, "$.artboards[0].nonesuch").unwrap_err().contains("nothing"), "{line}");
        assert!(schema.at(&doc, "layers[").unwrap_err().contains("RFC 9535"), "{line}");
    }
}

#[test]
fn an_unknown_type_is_refused_with_the_nearest_names() {
    for (line, schema, _) in both() {
        let err = schema.describe("artboard").unwrap_err();
        assert!(err.contains("Artboard"), "{line}: {err}");
        assert!(schema.describe("Nonesuch").unwrap_err().contains("index: true"), "{line}");
    }
}

#[test]
fn the_index_lists_every_type_once_with_a_line_each() {
    for (line, schema, _) in both() {
        let index = schema.index();
        assert_eq!(index["start"], "Document", "{line}");
        let mut names: Vec<&str> = index["types"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        names.sort_unstable();
        assert_eq!(names, ["Align", "Artboard", "Color", "Document", "Kind", "Node", "Rect"], "{line}");
        let doc = index["types"].as_array().unwrap().iter().find(|t| t["name"] == "Document").unwrap();
        assert_eq!(doc["description"], "A drawing: its artboards and the art on them", "{line}");
    }
}

#[test]
fn every_reference_in_the_schema_resolves() {
    fn walk(v: &Value, schema: &Schema, missing: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                if let Some(r) = o.get("$ref").and_then(Value::as_str) {
                    let name = r.strip_prefix("#/definitions/").or_else(|| r.strip_prefix("#/$defs/"));
                    if name.is_none_or(|n| !schema.types().contains_key(n)) {
                        missing.push(r.into());
                    }
                }
                o.values().for_each(|v| walk(v, schema, missing));
            }
            Value::Array(a) => a.iter().for_each(|v| walk(v, schema, missing)),
            _ => {}
        }
    }
    for (line, schema, _) in both() {
        let mut missing = vec![];
        schema.types().values().for_each(|v| walk(v, &schema, &mut missing));
        assert!(missing.is_empty(), "{line}: {missing:?}");
    }
}

#[test]
fn a_tools_arguments_choose_the_question() {
    for (line, schema, doc) in both() {
        let no_document = || -> Result<Value, String> { panic!("{line}: the document is read only for `at`") };
        assert_eq!(schema.answer(&json!({}), no_document).unwrap()["type"], "Document", "{line}");
        assert_eq!(schema.answer(&json!({"type": "Rect"}), no_document).unwrap()["type"], "Rect", "{line}");
        assert_eq!(schema.answer(&json!({"index": true}), no_document).unwrap()["start"], "Document", "{line}");
        assert_eq!(schema.answer(&json!({"at": "$.artboards[0]"}), || Ok(doc.clone())).unwrap()["type"], "Artboard", "{line}");
        assert!(schema.answer(&json!({"type": "Rect", "at": "$"}), no_document).unwrap_err().contains("not both"), "{line}");
        assert!(schema.answer(&json!({"type": 3}), no_document).unwrap_err().contains("`type` must be a string"), "{line}");
    }
}

#[test]
fn a_schema_that_is_not_one_has_only_an_empty_root() {
    let schema = Schema::new(json!("nonsense"), "Model");
    assert_eq!(schema.types().len(), 1);
    assert_eq!(schema.describe("Model").unwrap()["type"], "Model");
    assert!(schema.at(&json!({"a": 1}), "$.a").unwrap_err().contains("not a property"));
}
