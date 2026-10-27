//! The three tools' MCP definitions, the same in every app that offers them: `query_document`
//! ([`crate::query`]), `document_schema` ([`crate::Schema`]) and `patch_document`
//! ([`crate::Patch`]). The wording names the app's model and gives its own examples
//! ([`ModelText`]); the names, arguments and answers are the crate's, so an agent that knows them
//! in one app knows them in all.

use serde_json::{Value, json};

use crate::query::{DEFAULT_LIMIT, size};

/// One tool as an MCP `tools/list` entry describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDef {
    /// The tool's name, as `tools/call` names it.
    pub name: &'static str,
    /// A short human-readable title.
    pub title: &'static str,
    /// What the tool does and answers, for the agent.
    pub description: String,
    /// The JSON Schema of the tool's arguments (an object; unknown keys refused).
    pub input_schema: Value,
    /// Whether the tool only reads (MCP's `readOnlyHint`).
    pub read_only: bool,
}

impl ToolDef {
    /// The `tools/list` entry: `{name, title, description, inputSchema, annotations: {readOnlyHint}}`.
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "title": self.title,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": {"readOnlyHint": self.read_only},
        })
    }
}

/// How the tools' descriptions speak of an app's model.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelText {
    /// The model as the descriptions name it, such as `document model`.
    pub model: String,
    /// Where the model's JSON is the same as the tools', such as `as document.json gives it`
    /// (empty for none).
    pub source: String,
    /// JSONPath examples into the model, such as `$.artboards[*].name`.
    pub query_examples: Vec<String>,
    /// The root type's name, as the schema has it ([`crate::Schema::root`]).
    pub root: String,
    /// Named types to give as examples, such as `Artboard`, `Node`.
    pub type_examples: Vec<String>,
    /// A JSON Pointer example into the model, such as `/layers/0/kind/children/2/opacity`.
    pub pointer_example: String,
    /// What else a patch does in this app, such as `An object added without an id gets a fresh
    /// one.` (empty for nothing).
    pub patch_note: String,
}

impl ModelText {
    /// ` (as document.json gives it)`-style source clause, or nothing.
    fn source(&self) -> String {
        if self.source.is_empty() { String::new() } else { format!(" {}", self.source) }
    }
}

/// The three tools, in the order an agent uses them: query, schema, patch; `max_bytes` is the
/// app's limit on a query's answer ([`crate::QueryOpts::max_bytes`]), which the description gives.
pub fn all(text: &ModelText, max_bytes: usize) -> Vec<ToolDef> {
    vec![query_document(text, max_bytes), document_schema(text), patch_document(text)]
}

/// `query_document`: JSONPath over the model ([`crate::query::from_params`]), its answer never
/// past `max_bytes` (the app's [`crate::QueryOpts::max_bytes`]).
pub fn query_document(text: &ModelText, max_bytes: usize) -> ToolDef {
    let examples = text.query_examples.join(", ");
    ToolDef {
        name: "query_document",
        title: "Query the document model",
        description: format!(
            "JSONPath (RFC 9535) over the {model}{source}, for one part of a large document{examples}. Answers {{matches: [{{path, pointer, value}}], total}}: each match's normalized path and its JSON Pointer (what patch_document takes), bounded: at most `limit` matches, each value cut at `depth` levels (containers below are summarized), never past {limit}. document_schema describes the model's types and the type at a path.",
            model = text.model,
            source = text.source(),
            examples = if examples.is_empty() { String::new() } else { format!(": {examples}") },
            limit = size(max_bytes),
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "The JSONPath, from $ (the document)"},
                "limit": {"type": "integer", "minimum": 0, "description": format!("At most this many matches (default {DEFAULT_LIMIT}; 0 counts only)")},
                "offset": {"type": "integer", "minimum": 0, "description": "Matches skipped first (default 0)"},
                "depth": {"type": "integer", "minimum": 0, "description": "Levels of each match's value shown; what lies below is summarized as {…n keys} or […n items] (default all)"}
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        read_only: true,
    }
}

/// `document_schema`: the model's schema a type at a time ([`crate::Schema::answer`]).
pub fn document_schema(text: &ModelText) -> ToolDef {
    let types = if text.type_examples.is_empty() { text.root.clone() } else { text.type_examples.join(", ") };
    ToolDef {
        name: "document_schema",
        title: "Describe the document model",
        description: format!(
            "The {model}'s schema, generated from the model's own types and doc comments, one type at a time: no argument describes {root} (the root), `type` one named type (its properties with their types, defaults and descriptions, and `types`, the named types to describe next), `at` the type of the value at a JSONPath into the active document (an enum resolved to the variant the value is), `index: true` every type with a line each.",
            model = text.model,
            root = text.root,
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "type": {"type": "string", "description": format!("A type of the model, such as {types} (default {})", text.root)},
                "at": {"type": "string", "description": "A JSONPath into the active document; describes the type of its first match"},
                "index": {"type": "boolean", "description": "List every type of the model with a line each"}
            },
            "additionalProperties": false
        }),
        read_only: true,
    }
}

/// `patch_document`: JSON Patch over the model ([`crate::Patch::from_params`]); the app loads the
/// result and records it as one undo step named `label`.
pub fn patch_document(text: &ModelText) -> ToolDef {
    let pointer = if text.pointer_example.is_empty() { "/items/0/name".to_string() } else { text.pointer_example.clone() };
    let note = if text.patch_note.is_empty() { String::new() } else { format!(" {}", text.patch_note) };
    ToolDef {
        name: "patch_document",
        title: "Patch the document model",
        description: format!(
            "JSON Patch (RFC 6902) with JSON Pointer paths (RFC 6901) over the {model}{source}, at the pointers query_document answers: operations add, remove, replace, move, copy and test, applied in order, all or none, the result loaded through the model (what it refuses is refused whole). One undo step, named `label`.{note}",
            model = text.model,
            source = text.source(),
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "array",
                    "minItems": 1,
                    "items": {"type": "object", "description": format!("{{op: add|remove|replace|move|copy|test, path: a JSON Pointer such as {pointer} (/- appends), value?, from?}}")},
                    "description": "The operations, in order"
                },
                "label": {"type": "string", "description": "The undo step's name (default Patch)"}
            },
            "required": ["patch"],
            "additionalProperties": false
        }),
        read_only: false,
    }
}
