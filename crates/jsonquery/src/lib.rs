#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

//! Read and change a large JSON model a part at a time, by the standards agents already know.
//!
//! An app's document model is often megabytes of JSON: far too much to hand an agent whole. This
//! crate answers one question at a time over the model's JSON (the `serde_json::Value` the app
//! serializes its model to), so an MCP server or control channel can offer the same three tools
//! in every Craft app:
//!
//! - [`query`]: JSONPath ([RFC 9535](https://www.rfc-editor.org/rfc/rfc9535)) over the model,
//!   each match with its normalized path and its JSON Pointer, bounded by `limit`, `offset`,
//!   `depth` (what lies below is summarized, never silently dropped) and the app's byte limit,
//!   kept exactly in the layout the app writes the answer in.
//! - [`patch`]: JSON Patch ([RFC 6902](https://www.rfc-editor.org/rfc/rfc6902)) at JSON Pointers
//!   ([RFC 6901](https://www.rfc-editor.org/rfc/rfc6901)), the pointers a query answers; applied
//!   in order, all or none. The app loads the result through its model, which refuses what the
//!   model cannot be, and records the change as one edit (an undo step).
//! - [`schema`]: the model's JSON Schema, read one type at a time: the root, one named type with
//!   its properties' types, defaults and descriptions, the type of the value at a JSONPath into a
//!   live document (an enum resolved to the variant the value is), or an index of every type. The
//!   app generates the schema from its own types and doc comments (`schemars`, 0.8 or 1.x), so it
//!   never lags the format.
//! - [`tools`]: the three tools' MCP definitions (names, descriptions, input schemas), worded for
//!   the app's model and its examples.
//!
//! What stays in the app: serializing its model to JSON and loading it back, any fix-up a patched
//! model needs before it loads (fresh ids for added objects, say), and the undo step.
//!
//! No I/O, no app code; compiles for `wasm32-unknown-unknown`.

pub mod patch;
pub mod query;
pub mod schema;
pub mod tools;

pub use patch::Patch;
pub use query::{Layout, QueryOpts};
pub use schema::Schema;
pub use tools::{ModelText, ToolDef};

/// The README's example, run as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
