//! A model's JSON Schema, read one type at a time.
//!
//! The app generates the schema from its model's own types and their doc comments (`schemars`
//! on its root type), so it never lags the format, and hands it to [`Schema::new`] as JSON. A
//! whole schema of a large model is far too much to hand an agent at once, so a [`Schema`]
//! answers a question at a time: [`Schema::describe`] one named type (its properties, each with
//! its type, default and description, and the names of the types they refer to, to describe
//! next), [`Schema::at`] the type of the value at a JSONPath into a live document (the variant
//! of an enum chosen by the value there), and [`Schema::index`] every type with a line each.
//!
//! Both schemars lines are read: 0.8 keeps the named types under `definitions` and refers to
//! them as `#/definitions/<name>`, 1.x under `$defs` as `#/$defs/<name>`.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};
use serde_json_path::{JsonPath, PathElement};

/// Where a schema's references point: schemars 0.8's `definitions`, then 1.x's `$defs`.
const REF_PREFIXES: [&str; 2] = ["#/definitions/", "#/$defs/"];

/// A model's schema, by type name, with the root among them.
#[derive(Clone, Debug, PartialEq)]
pub struct Schema {
    root: String,
    types: Map<String, Value>,
}

impl Schema {
    /// The schema whose root (the schema of the model as a whole, as `schemars::schema_for!`
    /// gives it, turned into JSON) is `root_schema`, its root type named `root`. Its named types
    /// are those under `definitions` or `$defs`; the root joins them as `root`.
    pub fn new(root_schema: Value, root: &str) -> Schema {
        let mut root_schema = match root_schema {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        let mut types = Map::new();
        for key in ["definitions", "$defs"] {
            if let Some(Value::Object(defs)) = root_schema.remove(key) {
                types.extend(defs);
            }
        }
        root_schema.remove("$schema");
        root_schema.remove("title");
        types.insert(root.to_string(), Value::Object(root_schema));
        Schema { root: root.to_string(), types }
    }

    /// The root type's name: where a description of the model starts.
    pub fn root(&self) -> &str {
        &self.root
    }

    /// Every type's schema by name, the root among them.
    pub fn types(&self) -> &Map<String, Value> {
        &self.types
    }

    /// `{start, types: [{name, description}]}`: every type, with the first line of its
    /// description.
    pub fn index(&self) -> Value {
        let types: Vec<Value> = self
            .types
            .iter()
            .map(|(name, schema)| {
                let line = description(schema).and_then(|d| d.split_terminator(['.', '\n']).next().map(str::trim)).unwrap_or("");
                json!({"name": name, "description": line})
            })
            .collect();
        json!({"start": self.root, "types": types})
    }

    /// The type named `name`, rendered as [`Schema::render`] does, or why not, with the nearest
    /// names.
    pub fn describe(&self, name: &str) -> Result<Value, String> {
        if let Some(schema) = self.types.get(name) {
            return Ok(self.render(name, schema));
        }
        let needle = name.to_lowercase();
        let near: Vec<&str> = self.types.keys().filter(|k| k.to_lowercase().contains(&needle)).take(5).map(String::as_str).collect();
        Err(match near.is_empty() {
            true => format!("no type `{name}`; start from `{}`, or `index: true` lists every type", self.root),
            false => format!("no type `{name}`; did you mean {}?", near.join(", ")),
        })
    }

    /// The type of the value at `path` (its first match) in `document`, the model's JSON: the
    /// schema is walked beside the value, so an enum is the variant the value is.
    pub fn at(&self, document: &Value, path: &str) -> Result<Value, String> {
        let parsed = JsonPath::parse(path).map_err(|e| format!("`at` is not a JSONPath (RFC 9535): {e}"))?;
        let located = parsed.query_located(document);
        let Some(first) = located.iter().next() else { return Err(format!("nothing in the document matches `{path}`")) };
        let mut value = document;
        let mut schema = self.types.get(&self.root).ok_or("the model has no schema")?;
        let mut name = Some(self.root.clone());
        let mut walked = String::from("$");
        for element in first.location().iter() {
            let (resolved, resolved_name) = self.resolve(schema, value);
            let here = name.clone().or(resolved_name).unwrap_or_else(|| walked.clone());
            (value, schema) = step(element, value, resolved, &here, &mut walked)?;
            name = None;
        }
        let (resolved, resolved_name) = self.resolve(schema, value);
        let mut out = self.render(&name.or(resolved_name).unwrap_or_else(|| walked.clone()), resolved);
        if let Value::Object(o) = &mut out {
            o.insert("path".into(), Value::String(first.location().to_string()));
            o.insert("matches".into(), json!(located.len()));
            o.insert("valueType".into(), Value::String(json_type(value).into()));
        }
        Ok(out)
    }

    /// A tool's arguments, `{type?, at?, index?}` (the `document_schema` tool's input,
    /// [`crate::tools::document_schema`]), answered: `index` lists every type, `at` describes the
    /// type at a JSONPath into the document `document` gives (read only then), `type` one named
    /// type, and nothing the root.
    pub fn answer(&self, params: &Value, document: impl FnOnce() -> Result<Value, String>) -> Result<Value, String> {
        let text = |key: &str| match params.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(v)) => Ok(Some(v.as_str())),
            Some(_) => Err(format!("`{key}` must be a string")),
        };
        let (type_name, at) = (text("type")?, text("at")?);
        if params.get("index").and_then(Value::as_bool) == Some(true) {
            return Ok(self.index());
        }
        match (type_name, at) {
            (Some(_), Some(_)) => Err("give `type` or `at`, not both".into()),
            (_, Some(at)) => self.at(&document()?, at),
            (type_name, None) => self.describe(type_name.unwrap_or(&self.root)),
        }
    }

    /// `schema` with references followed and, for a choice of schemas, the one `value` is: a
    /// null takes the null branch, a tagged object the variant with its tag, anything else the
    /// first branch of its JSON type. Answers the schema and the name of the type it came from.
    fn resolve<'a>(&'a self, schema: &'a Value, value: &Value) -> (&'a Value, Option<String>) {
        let (schema, name) = self.deref(schema);
        let Some(branches) = choices(schema) else { return (schema, name) };
        let tag = value.get("type").and_then(Value::as_str);
        let pick = branches.iter().find(|b| {
            let (b, _) = self.deref(b);
            match (value, tag) {
                (Value::Null, _) => b.get("type") == Some(&json!("null")),
                (Value::Object(_), Some(tag)) if tag_values(b).is_some() => tag_values(b).is_some_and(|e| e.iter().any(|v| v.as_str() == Some(tag))),
                _ => types_of(b).iter().any(|t| t == json_type(value)),
            }
        });
        match pick {
            Some(b) => {
                let (b, branch_name) = self.deref(b);
                (b, branch_name.or(name))
            }
            None => (schema, name),
        }
    }

    /// `schema` with a reference followed to its type, and the type's name. A described field
    /// refers to its type as `allOf: [{$ref}]` (schemars 0.8: a reference takes no description
    /// beside it), so a lone `allOf` is looked through too.
    fn deref<'a>(&'a self, schema: &'a Value) -> (&'a Value, Option<String>) {
        if let Some(name) = ref_name(schema) {
            return match self.types.get(name) {
                Some(def) => (def, Some(name.to_string())),
                None => (schema, None),
            };
        }
        match schema.get("allOf").and_then(Value::as_array).map(Vec::as_slice) {
            Some([part]) if schema.get("properties").is_none() => self.deref(part),
            _ => (schema, None),
        }
    }

    /// One type for an agent: `{type, description, tag?, properties: [{name, type, required?,
    /// default?, description}], values?: [...] (a string enum), variants?: [...] (a choice of
    /// shapes), shape? (anything else, as a type expression), types: [the named types the
    /// properties refer to]}`.
    pub fn render(&self, name: &str, schema: &Value) -> Value {
        let mut refs = BTreeSet::new();
        let mut out = Map::new();
        out.insert("type".into(), Value::String(name.into()));
        if let Some(d) = description(schema) {
            out.insert("description".into(), Value::String(d.into()));
        }
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            // An internally tagged variant: its tag as `tag`, not as a property with one value.
            let tag = tag_values(schema).filter(|e| e.len() == 1).and_then(|e| e.first().copied()).and_then(Value::as_str);
            if let Some(tag) = tag {
                out.insert("tag".into(), Value::String(tag.into()));
            }
            out.insert("properties".into(), properties(props, schema, &mut refs, tag.map(|_| "type")));
        }
        if let Some(values) = schema.get("enum") {
            out.insert("values".into(), values.clone());
        }
        if let Some(branches) = choices(schema) {
            let variants: Vec<Value> = branches.iter().map(|b| self.variant(b, &mut refs)).collect();
            out.insert("variants".into(), Value::Array(variants));
        }
        if schema.get("properties").is_none() && schema.get("enum").is_none() && choices(schema).is_none() {
            out.insert("shape".into(), Value::String(type_expr(schema, &mut refs)));
        }
        refs.remove(name);
        out.insert("types".into(), Value::Array(refs.into_iter().map(Value::String).collect()));
        Value::Object(out)
    }

    /// One branch of a choice: a tagged object as `{tag, description?, properties}`, a string
    /// enum as `{values}`, anything else as `{type}`.
    fn variant(&self, branch: &Value, refs: &mut BTreeSet<String>) -> Value {
        let (b, name) = self.deref(branch);
        let tag = tag_values(b).and_then(|e| e.first().copied()).and_then(Value::as_str);
        let mut o = Map::new();
        match (tag, b.get("properties").and_then(Value::as_object), b.get("enum")) {
            (Some(tag), Some(props), _) => {
                o.insert("tag".into(), Value::String(tag.into()));
                o.insert("properties".into(), properties(props, b, refs, Some("type")));
            }
            (None, Some(props), _) => {
                o.insert("properties".into(), properties(props, b, refs, None));
            }
            (_, None, Some(values)) => {
                o.insert("values".into(), values.clone());
            }
            _ => {
                o.insert("type".into(), Value::String(type_expr(branch, refs)));
            }
        }
        if let Some(name) = name {
            o.insert("type".into(), Value::String(name));
        }
        if let Some(d) = description(b) {
            o.insert("description".into(), Value::String(d.into()));
        }
        Value::Object(o)
    }
}

/// One step of a located path: the value and the schema one key or index down from `value` and
/// `resolved` (its schema, references followed), `walked` the path so far (`here` names it).
fn step<'v, 's>(
    element: &PathElement<'_>,
    value: &'v Value,
    resolved: &'s Value,
    here: &str,
    walked: &mut String,
) -> Result<(&'v Value, &'s Value), String> {
    match element {
        PathElement::Name(key) => {
            walked.push_str(&format!("['{key}']"));
            let value = value.get(key).ok_or_else(|| format!("nothing at {walked}"))?;
            let schema = resolved
                .get("properties")
                .and_then(|p| p.get(key))
                .or_else(|| resolved.get("additionalProperties").filter(|a| a.is_object()))
                .ok_or_else(|| format!("`{key}` is not a property of {here}"))?;
            Ok((value, schema))
        }
        PathElement::Index(i) => {
            walked.push_str(&format!("[{i}]"));
            let value = value.get(i).ok_or_else(|| format!("nothing at {walked}"))?;
            // Items by position: schemars 0.8's `items: [...]`, or 1.x's `prefixItems`.
            let schema = match (resolved.get("items"), resolved.get("prefixItems")) {
                (Some(Value::Array(items)), _) | (_, Some(Value::Array(items))) => items.get(*i).ok_or_else(|| format!("{here} has no item {i}"))?,
                (Some(items), _) => items,
                (None, _) => return Err(format!("{here} is not an array")),
            };
            Ok((value, schema))
        }
    }
}

/// The name of the type `schema` refers to, when it is a reference to one.
fn ref_name(schema: &Value) -> Option<&str> {
    let r = schema.get("$ref").and_then(Value::as_str)?;
    REF_PREFIXES.iter().find_map(|p| r.strip_prefix(p))
}

/// The branches of a choice of schemas (`oneOf` or `anyOf`).
fn choices(schema: &Value) -> Option<&Vec<Value>> {
    schema.get("oneOf").or_else(|| schema.get("anyOf")).and_then(Value::as_array)
}

/// The values an internally tagged variant's `type` property may take: `enum` (schemars 0.8) or
/// `const` (1.x).
fn tag_values(schema: &Value) -> Option<Vec<&Value>> {
    let t = schema.get("properties")?.get("type")?;
    match (t.get("enum"), t.get("const")) {
        (Some(Value::Array(e)), _) => Some(e.iter().collect()),
        (_, Some(c)) => Some(vec![c]),
        _ => None,
    }
}

/// The JSON types a schema admits, as its `type` names them.
fn types_of(schema: &Value) -> Vec<String> {
    match schema.get("type") {
        Some(Value::String(t)) => vec![t.clone()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).map(String::from).collect(),
        _ => vec![],
    }
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn description(schema: &Value) -> Option<&str> {
    schema.get("description").and_then(Value::as_str)
}

fn properties(props: &Map<String, Value>, schema: &Value, refs: &mut BTreeSet<String>, skip: Option<&str>) -> Value {
    let required: BTreeSet<&str> = schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let list: Vec<Value> = props
        .iter()
        .filter(|(k, _)| Some(k.as_str()) != skip)
        .map(|(k, p)| {
            let mut o = Map::new();
            o.insert("name".into(), Value::String(k.clone()));
            o.insert("type".into(), Value::String(type_expr(p, refs)));
            if required.contains(k.as_str()) {
                o.insert("required".into(), Value::Bool(true));
            }
            if let Some(d) = p.get("default") {
                o.insert("default".into(), d.clone());
            }
            if let Some(d) = description(p) {
                o.insert("description".into(), Value::String(d.into()));
            }
            Value::Object(o)
        })
        .collect();
    Value::Array(list)
}

/// A schema as one readable type expression: `Artboard`, `array of Node`, `map of ImageBlob`,
/// `number | null`, `one of: left | right`, `object {x, y}`.
fn type_expr(schema: &Value, refs: &mut BTreeSet<String>) -> String {
    if let Some(name) = ref_name(schema) {
        refs.insert(name.to_string());
        return name.to_string();
    }
    if let Some(branches) = choices(schema) {
        let parts: Vec<String> = branches.iter().map(|b| type_expr(b, refs)).collect();
        return if parts.len() > 8 { format!("one of {} kinds", parts.len()) } else { parts.join(" | ") };
    }
    if let Some(parts) = schema.get("allOf").and_then(Value::as_array) {
        return parts.iter().map(|p| type_expr(p, refs)).collect::<Vec<_>>().join(" & ");
    }
    let types = types_of(schema);
    let nullable = types.iter().any(|t| t == "null");
    let base = match types.iter().find(|t| *t != "null").map(String::as_str) {
        Some("array") => array_expr(schema, refs),
        Some("object") => object_expr(schema, refs),
        Some("string") => match schema.get("enum").and_then(Value::as_array) {
            Some(values) => {
                format!("one of: {}", values.iter().map(|v| v.as_str().map_or_else(|| v.to_string(), String::from)).collect::<Vec<_>>().join(" | "))
            }
            None => "string".into(),
        },
        Some(t) => t.to_string(),
        None => match schema.get("enum").and_then(Value::as_array) {
            Some(values) => format!("one of: {}", values.iter().map(Value::to_string).collect::<Vec<_>>().join(" | ")),
            None => "any".into(),
        },
    };
    if nullable && !types.iter().all(|t| t == "null") { format!("{base} | null") } else { base }
}

/// Longest fixed-length array written out item by item (`[number, number]`); a longer one is
/// `array of 16 number`.
const SHORT_TUPLE: u64 = 4;

/// An array schema's type expression: `array of Node`, or `[number, number]` for items by
/// position (schemars 0.8's `items: [...]`, 1.x's `prefixItems`) or a short fixed length
/// (`minItems` equal to `maxItems`, as both write `[f64; 2]`).
fn array_expr(schema: &Value, refs: &mut BTreeSet<String>) -> String {
    let tuple =
        |items: &[&Value], refs: &mut BTreeSet<String>| format!("[{}]", items.iter().map(|i| type_expr(i, refs)).collect::<Vec<_>>().join(", "));
    let fixed = match (schema.get("minItems").and_then(Value::as_u64), schema.get("maxItems").and_then(Value::as_u64)) {
        (Some(min), Some(max)) if min == max => Some(min),
        _ => None,
    };
    match (schema.get("items"), schema.get("prefixItems")) {
        (Some(Value::Array(items)), _) | (_, Some(Value::Array(items))) => tuple(&items.iter().collect::<Vec<_>>(), refs),
        (Some(item), _) => match fixed {
            Some(n) if n <= SHORT_TUPLE => tuple(&vec![item; usize::try_from(n).unwrap_or(0)], refs),
            Some(n) => format!("array of {n} {}", type_expr(item, refs)),
            None => format!("array of {}", type_expr(item, refs)),
        },
        (None, _) => "array".into(),
    }
}

/// An object schema's type expression: `object {x, y}` with its properties, `map of Rect` for a
/// map, else `object`.
fn object_expr(schema: &Value, refs: &mut BTreeSet<String>) -> String {
    match (schema.get("properties").and_then(Value::as_object), schema.get("additionalProperties")) {
        (Some(props), _) => format!("object {{{}}}", props.keys().cloned().collect::<Vec<_>>().join(", ")),
        (None, Some(ap)) if ap.is_object() => format!("map of {}", type_expr(ap, refs)),
        _ => "object".into(),
    }
}
