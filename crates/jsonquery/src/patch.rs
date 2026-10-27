//! JSON Patch ([RFC 6902](https://www.rfc-editor.org/rfc/rfc6902)) with JSON Pointer paths
//! ([RFC 6901](https://www.rfc-editor.org/rfc/rfc6901)) over a model's JSON, at the pointers a
//! [`crate::query`] answers (each match's `pointer`). The operations (`add`, `remove`, `replace`,
//! `move`, `copy`, `test`) apply in order, all or none.
//!
//! The app does the rest: it serializes its model, applies the patch, fixes up what its model
//! needs (fresh ids for objects added without one, say), loads the result through its model,
//! which refuses what the model cannot be, and records the change as one edit, so a patch is
//! undone and replayed like any other.

use serde_json::Value;

/// A parsed JSON Patch: one or more operations, applied in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Patch(Vec<json_patch::PatchOperation>);

impl Patch {
    /// The patch in `ops`, a non-empty array of operations, each with `op`, `path` (a JSON
    /// Pointer) and `value` or `from` as the op takes (the `patch_document` tool's `patch`
    /// argument, [`crate::tools::patch_document`]).
    pub fn parse(ops: &Value) -> Result<Patch, String> {
        match ops {
            Value::Array(a) if !a.is_empty() => {
                serde_json::from_value(ops.clone()).map(Patch).map_err(|e| format!("`patch` is not a JSON Patch (RFC 6902): {e}"))
            }
            _ => Err("`patch` must be a non-empty array of JSON Patch operations (RFC 6902), each with op (add, remove, replace, move, copy or test), path (a JSON Pointer) and value or from as the op takes".into()),
        }
    }

    /// The patch in a tool's arguments, `{patch, …}`.
    pub fn from_params(params: &Value) -> Result<Patch, String> {
        Patch::parse(params.get("patch").unwrap_or(&Value::Null))
    }

    /// How many operations the patch has.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the patch has no operations ([`Patch::parse`] refuses one, so never for a parsed
    /// patch).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Apply the patch to `value`: the number of operations applied, or why not (the failing
    /// operation's index, path and reason) with `value` as it was.
    pub fn apply(&self, value: &mut Value) -> Result<usize, String> {
        json_patch::patch(value, &self.0).map_err(|e| e.to_string())?;
        Ok(self.0.len())
    }
}
