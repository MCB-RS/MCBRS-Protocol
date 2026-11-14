//! Strict parser for the meta-schema used by Mojang's `bedrock-protocol-docs`.
//!
//! The files are JSON Schema (draft-07) documents extended with `x-*` keys that
//! describe the binary layout. The parser is deliberately strict: every key and
//! every enumerated value found by `scripts/audit_schema.py` is modelled here,
//! and anything else is a hard error. When Mojang introduces a new construct
//! the generator therefore fails loudly instead of silently emitting wrong code.

use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

#[derive(Debug)]
pub struct SchemaError {
    pub location: String,
    pub message: String,
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.location, self.message)
    }
}

impl std::error::Error for SchemaError {}

pub type Result<T> = std::result::Result<T, SchemaError>;

fn err<T>(location: &str, message: impl Into<String>) -> Result<T> {
    Err(SchemaError { location: location.to_owned(), message: message.into() })
}

/// `type` keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonType {
    Object,
    String,
    Integer,
    Boolean,
    Array,
    Number,
    Null,
}

/// `x-underlying-type` / `x-control-value-type` keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Underlying {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    Float,
    Double,
    Boolean,
    /// Seen on a couple of enums whose C++ underlying type was not resolved.
    Object,
}

impl Underlying {
    fn parse(location: &str, value: &str) -> Result<Self> {
        Ok(match value {
            "uint8" => Self::U8,
            "int8" => Self::I8,
            "uint16" => Self::U16,
            "int16" => Self::I16,
            "uint32" => Self::U32,
            "int32" => Self::I32,
            "uint64" => Self::U64,
            "int64" => Self::I64,
            "float" => Self::Float,
            "double" => Self::Double,
            "boolean" => Self::Boolean,
            "object" => Self::Object,
            other => return err(location, format!("unknown underlying type {other:?}")),
        })
    }
}

/// `x-serialization-options` keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    /// `"Compression"`: integers are written as varints.
    pub compression: bool,
    /// `"Enum-as-Value"`: the enum is written as its number, not its name.
    pub enum_as_value: bool,
    /// `"Allow unknown enum values"`.
    pub allow_unknown: bool,
    /// `"No size compression"`: the length prefix is a fixed `u32`.
    pub no_size_compression: bool,
    /// `"Big Endian"`.
    pub big_endian: bool,
}

impl Options {
    /// Options of an array that apply to its elements.
    pub fn for_items(mut self) -> Self {
        self.no_size_compression = false;
        self
    }

    /// Use-site options take precedence, the rest is inherited.
    pub fn or(self, inherited: Self) -> Self {
        Self {
            compression: self.compression || inherited.compression,
            enum_as_value: self.enum_as_value || inherited.enum_as_value,
            allow_unknown: self.allow_unknown || inherited.allow_unknown,
            no_size_compression: self.no_size_compression || inherited.no_size_compression,
            big_endian: self.big_endian || inherited.big_endian,
        }
    }
}

/// One schema node (a document root, a property, an array item, a branch...).
#[derive(Debug, Default)]
pub struct Node {
    pub description: Option<String>,
    pub ty: Option<JsonType>,
    /// Target of `$ref`, as a file stem (`"./Vec3.json"` -> `"Vec3"`).
    pub reference: Option<String>,
    /// `properties`, in document order.
    pub properties: Option<Vec<(String, Node)>>,
    pub required: Option<Vec<String>>,
    pub items: Option<Box<Node>>,
    pub additional_properties: Option<Box<Node>>,
    pub property_names: Option<Box<Node>>,
    pub one_of: Option<Vec<Node>>,
    /// `enum` (names) and `x-enum-binary-value` (numbers), always the same length.
    pub enumerators: Option<Vec<(String, i64)>>,
    pub enum_descriptions: BTreeMap<String, String>,
    pub underlying: Option<Underlying>,
    pub options: Options,
    pub control: Option<Underlying>,
    pub ordinal: Option<u64>,
    pub constant: Option<String>,
    pub min_items: Option<u64>,
    pub max_items: Option<u64>,
    pub max_length: Option<u64>,
    pub min_length: Option<u64>,
    pub max_properties: Option<u64>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub runtime_constraint: Option<String>,
    /// The property documents a `default`: the C++ member is a plain value
    /// with an initializer, not a `std::optional`.
    pub has_default: bool,
}

#[derive(Debug)]
pub struct PacketMeta {
    pub id: u32,
    pub details: Option<String>,
}

/// One JSON file.
#[derive(Debug)]
pub struct Document {
    /// C++ name of the type.
    pub title: String,
    pub minecraft_version: String,
    pub protocol_version: i64,
    pub packet: Option<PacketMeta>,
    pub root: Node,
}

fn as_str<'a>(location: &str, key: &str, value: &'a Value) -> Result<&'a str> {
    value.as_str().ok_or_else(|| SchemaError { location: location.to_owned(), message: format!("{key} must be a string") })
}

fn as_u64(location: &str, key: &str, value: &Value) -> Result<u64> {
    value.as_u64().ok_or_else(|| SchemaError {
        location: location.to_owned(),
        message: format!("{key} must be a non-negative integer"),
    })
}

fn as_object<'a>(location: &str, key: &str, value: &'a Value) -> Result<&'a Map<String, Value>> {
    value.as_object().ok_or_else(|| SchemaError { location: location.to_owned(), message: format!("{key} must be an object") })
}

fn as_array<'a>(location: &str, key: &str, value: &'a Value) -> Result<&'a Vec<Value>> {
    value.as_array().ok_or_else(|| SchemaError { location: location.to_owned(), message: format!("{key} must be an array") })
}

fn parse_ref(location: &str, value: &str) -> Result<String> {
    match value.strip_prefix("./").and_then(|rest| rest.strip_suffix(".json")) {
        Some(stem) if !stem.is_empty() && !stem.contains('/') => Ok(stem.to_owned()),
        _ => err(location, format!("unsupported $ref {value:?} (expected \"./<file>.json\")")),
    }
}

fn parse_node(location: &str, object: &Map<String, Value>, is_root: bool) -> Result<Node> {
    let mut node = Node::default();
    let mut enum_names: Option<Vec<String>> = None;
    let mut enum_values: Option<Vec<i64>> = None;

    for (key, value) in object {
        match key.as_str() {
            // Document-level metadata, handled by `parse_document`.
            "title" | "$schema" | "$id" | "x-format-version" | "x-minecraft-version" | "x-protocol-version"
            | "$metaProperties"
                if is_root => {}
            "description" => node.description = Some(as_str(location, key, value)?.to_owned()),
            "type" => {
                node.ty = Some(match as_str(location, key, value)? {
                    "object" => JsonType::Object,
                    "string" => JsonType::String,
                    "integer" => JsonType::Integer,
                    "boolean" => JsonType::Boolean,
                    "array" => JsonType::Array,
                    "number" => JsonType::Number,
                    "null" => JsonType::Null,
                    other => return err(location, format!("unknown type {other:?}")),
                })
            }
            "$ref" => node.reference = Some(parse_ref(location, as_str(location, key, value)?)?),
            "properties" => {
                let mut properties = Vec::new();
                for (name, child) in as_object(location, key, value)? {
                    let child_location = format!("{location}/{name}");
                    let child = as_object(&child_location, "property", child)?;
                    properties.push((name.clone(), parse_node(&child_location, child, false)?));
                }
                node.properties = Some(properties);
            }
            "required" => {
                let mut required = Vec::new();
                for item in as_array(location, key, value)? {
                    required.push(as_str(location, key, item)?.to_owned());
                }
                node.required = Some(required);
            }
            "items" | "additionalProperties" | "propertyNames" => {
                let child_location = format!("{location}/<{key}>");
                let child = parse_node(&child_location, as_object(&child_location, key, value)?, false)?;
                match key.as_str() {
                    "items" => node.items = Some(Box::new(child)),
                    "additionalProperties" => node.additional_properties = Some(Box::new(child)),
                    _ => node.property_names = Some(Box::new(child)),
                }
            }
            "oneOf" => {
                let mut branches = Vec::new();
                for (index, branch) in as_array(location, key, value)?.iter().enumerate() {
                    let child_location = format!("{location}/oneOf[{index}]");
                    branches.push(parse_node(&child_location, as_object(&child_location, key, branch)?, false)?);
                }
                if branches.is_empty() {
                    return err(location, "oneOf must not be empty");
                }
                node.one_of = Some(branches);
            }
            "enum" => {
                let mut names = Vec::new();
                for item in as_array(location, key, value)? {
                    names.push(as_str(location, key, item)?.to_owned());
                }
                enum_names = Some(names);
            }
            "x-enum-binary-value" => {
                let mut values = Vec::new();
                for item in as_array(location, key, value)? {
                    values.push(item.as_i64().ok_or_else(|| SchemaError {
                        location: location.to_owned(),
                        message: format!("enum value {item} does not fit in a signed 64-bit integer"),
                    })?);
                }
                enum_values = Some(values);
            }
            "$enumDescription" => {
                for (name, text) in as_object(location, key, value)? {
                    node.enum_descriptions.insert(name.clone(), as_str(location, key, text)?.to_owned());
                }
            }
            "x-underlying-type" => node.underlying = Some(Underlying::parse(location, as_str(location, key, value)?)?),
            "x-control-value-type" => node.control = Some(Underlying::parse(location, as_str(location, key, value)?)?),
            "x-serialization-options" => {
                for item in as_array(location, key, value)? {
                    match as_str(location, key, item)? {
                        "Compression" => node.options.compression = true,
                        "Enum-as-Value" => node.options.enum_as_value = true,
                        "Allow unknown enum values" => node.options.allow_unknown = true,
                        "No size compression" => node.options.no_size_compression = true,
                        "Big Endian" => node.options.big_endian = true,
                        other => return err(location, format!("unknown serialization option {other:?}")),
                    }
                }
            }
            "x-ordinal-index" => node.ordinal = Some(as_u64(location, key, value)?),
            "const" => node.constant = Some(as_str(location, key, value)?.to_owned()),
            "minItems" => node.min_items = Some(as_u64(location, key, value)?),
            "maxItems" => node.max_items = Some(as_u64(location, key, value)?),
            "maxLength" => node.max_length = Some(as_u64(location, key, value)?),
            "x-runtime-constraint-description" => {
                node.runtime_constraint = Some(as_str(location, key, value)?.to_owned())
            }
            // Value limits: they do not change the layout, but they are kept
            // so the generated code can check them (`Validate`).
            "minimum" | "maximum" => {
                let number = value.as_f64().ok_or_else(|| SchemaError {
                    location: location.to_owned(),
                    message: format!("{key} must be a number"),
                })?;
                if key == "minimum" {
                    node.minimum = Some(number);
                } else {
                    node.maximum = Some(number);
                }
            }
            "minLength" => node.min_length = Some(as_u64(location, key, value)?),
            "maxProperties" => node.max_properties = Some(as_u64(location, key, value)?),
            "pattern" => {
                as_str(location, key, value)?;
            }
            // The C++ default value; any JSON value. Only its presence matters.
            "default" => node.has_default = true,
            other => return err(location, format!("unknown schema key {other:?}")),
        }
    }

    match (enum_names, enum_values) {
        (Some(names), Some(values)) => {
            if names.len() != values.len() {
                return err(location, "enum and x-enum-binary-value have different lengths");
            }
            if names.is_empty() {
                return err(location, "enum must not be empty");
            }
            node.enumerators = Some(names.into_iter().zip(values).collect());
        }
        (None, None) => {}
        _ => return err(location, "enum and x-enum-binary-value must be used together"),
    }
    Ok(node)
}

pub fn parse_document(stem: &str, json: &Value) -> Result<Document> {
    let location = format!("{stem}.json");
    let object = as_object(&location, "document", json)?;
    let field = |key: &str| {
        object
            .get(key)
            .ok_or_else(|| SchemaError { location: location.clone(), message: format!("missing {key}") })
    };
    let title = as_str(&location, "title", field("title")?)?.to_owned();
    let minecraft_version = as_str(&location, "x-minecraft-version", field("x-minecraft-version")?)?.to_owned();
    let protocol_version = field("x-protocol-version")?.as_i64().ok_or_else(|| SchemaError {
        location: location.clone(),
        message: "x-protocol-version must be an integer".to_owned(),
    })?;

    let packet = match object.get("$metaProperties") {
        None => None,
        Some(meta) => {
            let meta = as_object(&location, "$metaProperties", meta)?;
            let mut id = None;
            let mut details = None;
            for (key, value) in meta {
                match key.as_str() {
                    "[cereal:packet]" => {
                        let raw = as_u64(&location, key, value)?;
                        id = Some(u32::try_from(raw).map_err(|_| SchemaError {
                            location: location.clone(),
                            message: format!("packet id {raw} does not fit in u32"),
                        })?);
                    }
                    "[cereal:packet_details]" => details = Some(as_str(&location, key, value)?.to_owned()),
                    other => return err(&location, format!("unknown $metaProperties key {other:?}")),
                }
            }
            match id {
                Some(id) => Some(PacketMeta { id, details }),
                None => return err(&location, "$metaProperties without [cereal:packet]"),
            }
        }
    };

    Ok(Document {
        title,
        minecraft_version,
        protocol_version,
        packet,
        root: parse_node(&location, object, true)?,
    })
}

/// Loads every `*.json` file of `dir`, keyed (and therefore ordered) by stem.
pub fn load_dir(dir: &Path) -> std::result::Result<BTreeMap<String, Document>, Box<dyn std::error::Error>> {
    let mut documents = BTreeMap::new();
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("non UTF-8 file name: {}", path.display()))?
            .to_owned();
        let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let json: Value = serde_json::from_str(&text).map_err(|e| format!("{}: invalid JSON: {e}", path.display()))?;
        documents.insert(stem.clone(), parse_document(&stem, &json)?);
    }
    if documents.is_empty() {
        return Err(format!("no JSON schema found in {}", dir.display()).into());
    }
    Ok(documents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc(extra: Value) -> Value {
        let mut base = json!({
            "title": "T", "$schema": "s", "$id": "/T.json", "x-format-version": "v",
            "x-minecraft-version": "1.0", "x-protocol-version": 1
        });
        base.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        base
    }

    #[test]
    fn rejects_unknown_keys_and_values() {
        assert!(parse_document("T", &doc(json!({"type": "object", "x-new-thing": 1}))).is_err());
        assert!(parse_document("T", &doc(json!({"type": "tuple"}))).is_err());
        assert!(parse_document("T", &doc(json!({"type": "integer", "x-underlying-type": "int128"}))).is_err());
        assert!(parse_document("T", &doc(json!({"type": "integer", "x-serialization-options": ["Zip"]}))).is_err());
        assert!(parse_document("T", &doc(json!({"$ref": "http://example.com/x.json"}))).is_err());
        assert!(parse_document("T", &doc(json!({"type": "string", "enum": ["A"]}))).is_err());
    }

    #[test]
    fn parses_packet_metadata_and_options() {
        let parsed = parse_document(
            "T",
            &doc(json!({
                "$ref": "./Payload.json",
                "$metaProperties": {"[cereal:packet]": 27, "[cereal:packet_details]": "d"}
            })),
        )
        .unwrap();
        assert_eq!(parsed.packet.as_ref().unwrap().id, 27);
        assert_eq!(parsed.root.reference.as_deref(), Some("Payload"));

        let parsed = parse_document(
            "T",
            &doc(json!({"type": "object", "properties": {"A": {
                "type": "integer", "x-underlying-type": "int32",
                "x-serialization-options": ["Compression", "Big Endian"], "x-ordinal-index": 0
            }}})),
        )
        .unwrap();
        let (name, field) = &parsed.root.properties.as_ref().unwrap()[0];
        assert_eq!(name, "A");
        assert!(field.options.compression && field.options.big_endian && !field.options.enum_as_value);
        assert_eq!(field.underlying, Some(Underlying::I32));
    }
}
