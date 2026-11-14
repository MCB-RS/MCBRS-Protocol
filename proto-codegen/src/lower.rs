//! Lowers parsed schema documents into the [`Model`].
//!
//! Wire rules applied here (inferred from the schemas, see the README):
//!
//! * object: its properties in `x-ordinal-index` order. A property missing
//!   from `required` is a `std::optional` (bool flag + value) unless it
//!   documents a `default`: then it is a plain member with an initializer,
//!   always present on the wire;
//! * integer: fixed little-endian, `"Compression"` -> varint, `"Big Endian"`;
//! * array: `VarUInt32` count (`u32` with `"No size compression"`), no count at
//!   all when `minItems == maxItems`; array-level options apply to the items;
//! * `additionalProperties`: a map, `VarUInt32` count then key/value pairs;
//! * enum: its number with `"Enum-as-Value"`, otherwise its name as a string;
//! * `oneOf`: the branch index as an `x-control-value-type` varint, then the
//!   branch; a `const` field of a branch is an ordinary field with a fixed
//!   value, written after the index;
//! * `string`: length-prefixed bytes (text and binary payloads alike);
//! * a property without `type` / `$ref` / `oneOf` is network NBT.

use crate::ir::*;
use crate::names::{self, Namer};
use crate::overrides;
use crate::schema::{Document, JsonType, Node, Options, Underlying};
use std::collections::{BTreeMap, HashMap};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Packet,
    Enum,
    Struct,
    Variant,
    Alias,
}

fn kind_of(doc: &Document) -> Kind {
    let root = &doc.root;
    if doc.packet.is_some() {
        Kind::Packet
    } else if root.enumerators.is_some() {
        Kind::Enum
    } else if root.one_of.is_some() {
        Kind::Variant
    } else if root.reference.is_none() && root.ty == Some(JsonType::Object) && root.additional_properties.is_none() {
        Kind::Struct
    } else {
        Kind::Alias
    }
}

/// Encoding hints flowing from a use site into what it refers to (array ->
/// items, `$ref` -> aliased primitive).
#[derive(Debug, Clone, Copy, Default)]
struct Inherit {
    underlying: Option<Underlying>,
    options: Options,
}

fn int_of(underlying: Underlying) -> Option<Int> {
    Some(match underlying {
        Underlying::U8 => Int::U8,
        Underlying::I8 => Int::I8,
        Underlying::U16 => Int::U16,
        Underlying::I16 => Int::I16,
        Underlying::U32 => Int::U32,
        Underlying::I32 => Int::I32,
        Underlying::U64 => Int::U64,
        Underlying::I64 => Int::I64,
        Underlying::Float | Underlying::Double | Underlying::Boolean | Underlying::Object => return None,
    })
}

/// Integer type of an enum. Mojang documents a couple of enums with an
/// unresolved (`object`) underlying type: those are single bytes.
fn enum_int(underlying: Option<Underlying>) -> Int {
    match underlying {
        None => Int::I32,
        Some(Underlying::Object | Underlying::Boolean) => Int::U8,
        Some(other) => int_of(other).unwrap_or(Int::I32),
    }
}

fn enc_of(options: Options) -> Enc {
    if options.compression {
        Enc::Var
    } else if options.big_endian {
        Enc::Be
    } else {
        Enc::Le
    }
}

fn normalize(name: &str) -> String {
    name.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

fn join_doc(parts: &[Option<String>]) -> Option<String> {
    let parts: Vec<&str> = parts.iter().flatten().map(String::as_str).filter(|part| !part.trim().is_empty()).collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

pub struct Lowerer<'a> {
    docs: &'a BTreeMap<String, Document>,
    /// Document stem -> Rust type name.
    rust_names: HashMap<&'a str, String>,
    types: Namer,
    /// Inline definitions already hoisted, keyed by node address, so that a
    /// node reached twice (through an alias) yields a single definition.
    hoisted: HashMap<usize, String>,
    /// Aliases being resolved, to detect self-referential ones.
    resolving: Vec<&'a str>,
    enum_index: HashMap<String, usize>,
    /// Indices of the field overrides that matched a schema property.
    used_overrides: std::collections::HashSet<usize>,
    model: Model,
}

impl<'a> Lowerer<'a> {
    pub fn new(docs: &'a BTreeMap<String, Document>) -> Self {
        Self {
            docs,
            rust_names: HashMap::new(),
            types: Namer::with_reserved(names::RESERVED_TYPE_NAMES),
            hoisted: HashMap::new(),
            resolving: Vec::new(),
            enum_index: HashMap::new(),
            used_overrides: std::collections::HashSet::new(),
            model: Model::default(),
        }
    }

    pub fn run(mut self) -> Result<Model> {
        let docs = self.docs;
        // Root documents get first pick of the names.
        for (stem, doc) in docs {
            let name = self.types.unique(&names::pascal_case(&doc.title));
            self.rust_names.insert(stem.as_str(), name);
        }
        if let Some(doc) = docs.values().max_by_key(|doc| doc.protocol_version) {
            self.model.protocol_version = doc.protocol_version;
            self.model.minecraft_version = doc.minecraft_version.clone();
        }

        // Enums first: constants of tagged structs refer to their variants.
        for (stem, doc) in docs {
            if kind_of(doc) == Kind::Enum {
                let name = self.rust_names[stem.as_str()].clone();
                let def = build_enum(name, &doc.title, &doc.root, stem)?;
                self.push_enum(def);
            }
        }

        let mut packet_variants = Namer::with_reserved(&["Self"]);
        for (stem, doc) in docs {
            let name = self.rust_names[stem.as_str()].clone();
            let location = format!("{stem}.json");
            match kind_of(doc) {
                Kind::Enum => {}
                Kind::Struct => {
                    let def = self.build_struct(name, &doc.title, &doc.root, &location)?;
                    self.model.structs.push(def);
                }
                Kind::Variant => {
                    let def = self.build_variant(name, &doc.title, &doc.root, &location)?;
                    self.model.variants.push(def);
                }
                Kind::Alias => {
                    self.resolving.push(stem.as_str());
                    let ty = self.lower(&doc.root, &name, Inherit::default(), &location);
                    self.resolving.pop();
                    let doc_text = join_doc(&[
                        doc.root.description.clone(),
                        doc.root.runtime_constraint.as_ref().map(|c| format!("Runtime constraint: {c}")),
                    ]);
                    self.model.aliases.push(AliasDef { name, source: doc.title.clone(), doc: doc_text, ty: ty? });
                }
                Kind::Packet => {
                    let meta =
                        doc.packet.as_ref().ok_or_else(|| format!("{location}: packet without $metaProperties"))?;
                    let payload = match &doc.root.reference {
                        Some(target) => self.lower_ref(target, None, Options::default(), &location)?,
                        None => self.lower(&doc.root, &format!("{name} Payload"), Inherit::default(), &location)?,
                    };
                    let short = name.strip_suffix("Packet").filter(|short| !short.is_empty()).unwrap_or(&name);
                    let variant = packet_variants.unique(short);
                    self.model.packets.push(PacketDef {
                        name,
                        variant,
                        source: doc.title.clone(),
                        id: meta.id,
                        doc: join_doc(&[doc.root.description.clone(), meta.details.clone()]),
                        payload,
                    });
                }
            }
        }

        // An override that matches nothing means the schema moved on: the
        // special case must be reviewed, not silently dropped.
        for (index, entry) in overrides::FIELD_OVERRIDES.iter().enumerate() {
            if !self.used_overrides.contains(&index) && docs.values().any(|doc| doc.title == entry.document) {
                return Err(format!(
                    "override for {}.{:?} matches no property (see proto-codegen/src/overrides.rs)",
                    entry.document, entry.property
                ));
            }
        }

        self.model.packets.sort_by_key(|packet| packet.id);
        for pair in self.model.packets.windows(2) {
            if pair[0].id == pair[1].id {
                return Err(format!(
                    "packets {} and {} share the id {}",
                    pair[0].source, pair[1].source, pair[0].id
                ));
            }
        }
        self.model.enums.sort_by(|a, b| a.name.cmp(&b.name));
        self.model.structs.sort_by(|a, b| a.name.cmp(&b.name));
        self.model.variants.sort_by(|a, b| a.name.cmp(&b.name));
        self.model.aliases.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(self.model)
    }

    fn push_enum(&mut self, def: EnumDef) {
        self.enum_index.insert(def.name.clone(), self.model.enums.len());
        self.model.enums.push(def);
    }

    fn doc(&self, stem: &str, location: &str) -> Result<(&'a str, &'a Document)> {
        let docs = self.docs;
        docs.get_key_value(stem)
            .map(|(key, doc)| (key.as_str(), doc))
            .ok_or_else(|| format!("{location}: $ref to unknown document {stem:?}"))
    }

    fn enum_ty(&self, name: String, declared: Option<Underlying>, options: Options) -> Ty {
        if options.enum_as_value {
            let wire = EnumWire::Raw(enum_int(declared), enc_of(options));
            Ty::Enum { name, wire, allow_unknown: options.allow_unknown }
        } else {
            // A name that is not in the enum cannot be represented, so
            // "allow unknown" only makes sense for numeric encodings.
            Ty::Enum { name, wire: EnumWire::Name, allow_unknown: false }
        }
    }

    fn lower_ref(
        &mut self,
        stem: &str,
        underlying: Option<Underlying>,
        options: Options,
        location: &str,
    ) -> Result<Ty> {
        let (stem, doc) = self.doc(stem, location)?;
        let name = self.rust_names[stem].clone();
        match kind_of(doc) {
            Kind::Enum => Ok(self.enum_ty(name, underlying.or(doc.root.underlying), options)),
            Kind::Struct | Kind::Variant => Ok(Ty::Named(name)),
            // Packet structs live in another module than the types; a type
            // that embeds a packet embeds its payload.
            Kind::Packet => match &doc.root.reference {
                Some(payload) if payload != stem => self.lower_ref(payload, underlying, options, &format!("{stem}.json")),
                _ => Err(format!("{location}: $ref to packet {stem:?}, which has no payload type of its own")),
            },
            Kind::Alias => {
                if self.resolving.contains(&stem) {
                    // The schema defines this type in terms of itself, which
                    // carries no layout information: treat it as an opaque
                    // string (every such type in the docs is textual).
                    return Ok(Ty::String);
                }
                self.resolving.push(stem);
                let target = self.lower(&doc.root, &name, Inherit { underlying, options }, &format!("{stem}.json"));
                self.resolving.pop();
                Ok(Ty::Alias { name, target: Box::new(target?) })
            }
        }
    }

    /// Lowers a node and attaches the limits documented on it, if any.
    fn lower(&mut self, node: &'a Node, hint: &str, inherit: Inherit, location: &str) -> Result<Ty> {
        let ty = self.lower_plain(node, hint, inherit, location)?;
        let constraints = constraints_for(&ty, node);
        Ok(if constraints.is_empty() { ty } else { Ty::Constrained(Box::new(ty), constraints) })
    }

    fn lower_plain(&mut self, node: &'a Node, hint: &str, inherit: Inherit, location: &str) -> Result<Ty> {
        let underlying = node.underlying.or(inherit.underlying);
        let options = node.options.or(inherit.options);

        if node.one_of.is_some() {
            return Ok(Ty::Named(self.hoist(node, hint, location, Self::build_variant_def)?));
        }
        if node.enumerators.is_some() {
            let key = node as *const Node as usize;
            let name = match self.hoisted.get(&key) {
                Some(name) => name.clone(),
                None => {
                    let name = self.types.unique(&names::pascal_case(hint));
                    let def = build_enum(name.clone(), hint, node, location)?;
                    self.push_enum(def);
                    self.hoisted.insert(key, name.clone());
                    name
                }
            };
            return Ok(self.enum_ty(name, underlying, options));
        }
        if let Some(target) = &node.reference {
            return self.lower_ref(target, underlying, options, location);
        }

        match node.ty {
            // No type information at all: Mojang leaves NBT payloads untyped.
            None => Ok(Ty::Nbt),
            Some(JsonType::Null) => Ok(Ty::Unit),
            Some(JsonType::Boolean) => Ok(Ty::Bool),
            Some(JsonType::String) => Ok(Ty::String),
            Some(JsonType::Number) => match underlying {
                Some(Underlying::Double) => Ok(Ty::F64),
                Some(Underlying::Float) | None => Ok(Ty::F32),
                Some(other) => Err(format!("{location}: number with underlying type {other:?}")),
            },
            Some(JsonType::Integer) => match underlying {
                // Only `ItemStackNetIdVariant` lacks a width: a varint32.
                None => Ok(Ty::Int(Int::I32, Enc::Var)),
                Some(underlying) => match int_of(underlying) {
                    Some(int) => Ok(Ty::Int(int, enc_of(options))),
                    None => Err(format!("{location}: integer with underlying type {underlying:?}")),
                },
            },
            Some(JsonType::Array) => {
                let items = node.items.as_deref().ok_or_else(|| format!("{location}: array without items"))?;
                let item_inherit = Inherit { underlying: inherit.underlying, options: options.for_items() };
                let item = Box::new(self.lower(items, hint, item_inherit, &format!("{location}/<items>"))?);
                match (node.min_items, node.max_items) {
                    (Some(min), Some(max)) if min == max => {
                        let len = usize::try_from(max).map_err(|_| format!("{location}: array too large"))?;
                        // Rust only derives `Default` for arrays up to 32 elements.
                        Ok(if len <= 32 { Ty::Array(item, len) } else { Ty::FixedVec(item, len) })
                    }
                    _ if options.no_size_compression => Ok(Ty::Vec(item, LenEnc::U32Le)),
                    _ => Ok(Ty::Vec(item, LenEnc::Var)),
                }
            }
            Some(JsonType::Object) => {
                if let Some(entry) = node.additional_properties.as_deref() {
                    self.lower_map(node, entry, hint, location)
                } else if node.properties.is_some() {
                    Ok(Ty::Named(self.hoist(node, hint, location, Self::build_struct_def)?))
                } else {
                    Ok(Ty::Unit)
                }
            }
        }
    }

    /// Maps come in two shapes: `additionalProperties` is the value type (the
    /// key is then a string, or `propertyNames`), or it is an object with
    /// exactly a `key` and a `value` property.
    fn lower_map(&mut self, node: &'a Node, entry: &'a Node, hint: &str, location: &str) -> Result<Ty> {
        let pair = entry.properties.as_ref().and_then(|properties| {
            let key = properties.iter().find(|(name, _)| name == "key");
            let value = properties.iter().find(|(name, _)| name == "value");
            match (key, value, properties.len()) {
                (Some((_, key)), Some((_, value)), 2) if entry.reference.is_none() => Some((key, value)),
                _ => None,
            }
        });
        let inherit = Inherit::default();
        let (key, value) = match pair {
            Some((key, value)) => (
                self.lower(key, &format!("{hint} Key"), inherit, &format!("{location}/<key>"))?,
                self.lower(value, &format!("{hint} Value"), inherit, &format!("{location}/<value>"))?,
            ),
            None => {
                let key = match node.property_names.as_deref() {
                    Some(names) => self.lower(names, &format!("{hint} Key"), inherit, &format!("{location}/<key>"))?,
                    None => Ty::String,
                };
                (key, self.lower(entry, &format!("{hint} Value"), inherit, &format!("{location}/<value>"))?)
            }
        };
        Ok(Ty::Map(Box::new(key), Box::new(value)))
    }

    /// Gives an inline definition a name of its own, once.
    fn hoist(
        &mut self,
        node: &'a Node,
        hint: &str,
        location: &str,
        build: fn(&mut Self, String, &str, &'a Node, &str) -> Result<()>,
    ) -> Result<String> {
        let key = node as *const Node as usize;
        if let Some(name) = self.hoisted.get(&key) {
            return Ok(name.clone());
        }
        let name = self.types.unique(&names::pascal_case(hint));
        self.hoisted.insert(key, name.clone());
        build(self, name.clone(), hint, node, location)?;
        Ok(name)
    }

    fn build_struct_def(&mut self, name: String, source: &str, node: &'a Node, location: &str) -> Result<()> {
        let def = self.build_struct(name, source, node, location)?;
        self.model.structs.push(def);
        Ok(())
    }

    fn build_variant_def(&mut self, name: String, source: &str, node: &'a Node, location: &str) -> Result<()> {
        let def = self.build_variant(name, source, node, location)?;
        self.model.variants.push(def);
        Ok(())
    }

    /// Resolves a `const` value to the enum variant it designates.
    fn const_variant(&self, enum_name: &str, constant: &str, location: &str) -> Result<String> {
        let def = self
            .enum_index
            .get(enum_name)
            .and_then(|&index| self.model.enums.get(index))
            .ok_or_else(|| format!("{location}: const refers to unknown enum {enum_name}"))?;
        // `const` values are spelled differently from the enumerators
        // ("changeentity" for `ChangeEntity`): compare letters and digits only.
        let wanted = normalize(constant);
        let mut matches: Vec<&str> = def
            .variants
            .iter()
            .filter(|variant| normalize(&variant.original) == wanted)
            .map(|variant| variant.name.as_str())
            .chain(
                def.aliases
                    .iter()
                    .filter(|alias| normalize(&alias.original) == wanted)
                    .map(|alias| alias.target.as_str()),
            )
            .collect();
        matches.sort_unstable();
        matches.dedup();
        match matches.as_slice() {
            [variant] => Ok((*variant).to_owned()),
            [] => Err(format!("{location}: const {constant:?} is not an enumerator of {}", def.source)),
            several => Err(format!(
                "{location}: const {constant:?} is ambiguous in {}: it matches {}",
                def.source,
                several.join(", ")
            )),
        }
    }

    fn build_struct(&mut self, name: String, source: &str, node: &'a Node, location: &str) -> Result<StructDef> {
        let mut properties: Vec<&'a (String, Node)> =
            node.properties.as_ref().map(|properties| properties.iter().collect()).unwrap_or_default();
        // Wire order; properties without an ordinal keep their document order.
        properties.sort_by_key(|(_, property)| property.ordinal.unwrap_or(u64::MAX));

        // Two properties at the same position would make the wire order a guess.
        for pair in properties.windows(2) {
            if let (Some(a), Some(b)) = (pair[0].1.ordinal, pair[1].1.ordinal) {
                if a == b {
                    return Err(format!(
                        "{location}: properties {:?} and {:?} share x-ordinal-index {a}",
                        pair[0].0, pair[1].0
                    ));
                }
            }
        }

        let required = node.required.as_deref().unwrap_or(&[]);
        for entry in required {
            if !properties.iter().any(|(property, _)| property == entry) {
                return Err(format!("{location}: required property {entry:?} is not defined"));
            }
        }

        let mut field_names = Namer::default();
        let mut const_names = Namer::default();
        let mut fields = Vec::new();
        for (original, property) in properties {
            let field_location = format!("{location}/{original}");
            let doc = join_doc(&[
                property.description.clone(),
                property.runtime_constraint.as_ref().map(|c| format!("Runtime constraint: {c}")),
            ]);
            let field_name = field_names.unique(&names::snake_case(original));
            let hint = format!("{name} {original}");

            if let Some(constant) = &property.constant {
                let ty = match self.lower(property, &hint, Inherit::default(), &field_location)? {
                    Ty::Enum { name, wire, .. } => Ty::Enum { name, wire, allow_unknown: false },
                    other => {
                        return Err(format!("{field_location}: const on a non-enum field ({other:?}) is not supported"))
                    }
                };
                let enum_name = match &ty {
                    Ty::Enum { name, .. } => name.clone(),
                    other => return Err(format!("{field_location}: const on a non-enum field ({other:?})")),
                };
                let variant = self.const_variant(&enum_name, constant, &field_location)?;
                let constant = ConstValue {
                    enum_name,
                    variant,
                    const_name: const_names.unique(&names::screaming_snake_case(original)),
                };
                fields.push(Field { name: field_name, original: original.clone(), doc, ty, constant: Some(constant) });
                continue;
            }

            let mut ty = match overrides::find(source, original) {
                Some((index, entry)) => {
                    // The override replaces a plain string; anything else
                    // means the schema changed and the override is stale.
                    let plain_string = property.ty == Some(JsonType::String)
                        && property.reference.is_none()
                        && property.enumerators.is_none();
                    if !plain_string {
                        return Err(format!(
                            "{field_location}: overridden as codec::{} but no longer a plain string",
                            entry.codec_type
                        ));
                    }
                    self.used_overrides.insert(index);
                    Ty::Extern(entry.codec_type.to_owned())
                }
                None => self.lower(property, &hint, Inherit::default(), &field_location)?,
            };
            // Not required and no default: a `std::optional`. Not required
            // with a default: a plain member, always written.
            if !required.contains(original) && !property.has_default {
                ty = Ty::Option(Box::new(ty));
            }
            fields.push(Field { name: field_name, original: original.clone(), doc, ty, constant: None });
        }

        let doc = join_doc(&[
            node.description.clone(),
            node.runtime_constraint.as_ref().map(|c| format!("Runtime constraint: {c}")),
        ]);
        Ok(StructDef { name, source: source.to_owned(), doc, fields })
    }

    fn build_variant(&mut self, name: String, source: &str, node: &'a Node, location: &str) -> Result<VariantDef> {
        let branches = node.one_of.as_ref().ok_or_else(|| format!("{location}: variant without oneOf"))?;

        let mut base_names = Vec::new();
        let mut tys = Vec::new();
        for (index, branch) in branches.iter().enumerate() {
            let branch_location = format!("{location}/oneOf[{index}]");
            let base = match &branch.reference {
                Some(stem) => {
                    let (stem, _) = self.doc(stem, &branch_location)?;
                    self.rust_names[stem].clone()
                }
                None if branch.one_of.is_some() => "Variant".to_owned(),
                None if branch.enumerators.is_some() => "Enum".to_owned(),
                None => match branch.ty {
                    None => "Nbt",
                    Some(JsonType::Null) => "Null",
                    Some(JsonType::Boolean) => "Bool",
                    Some(JsonType::Integer) => "Int",
                    Some(JsonType::Number) if branch.underlying == Some(Underlying::Double) => "Double",
                    Some(JsonType::Number) => "Float",
                    Some(JsonType::String) => "String",
                    Some(JsonType::Array) => "List",
                    Some(JsonType::Object) => "Object",
                }
                .to_owned(),
            };
            let hint = format!("{name} {base}");
            tys.push(self.lower(branch, &hint, Inherit::default(), &branch_location)?);
            base_names.push((base, branch.reference.is_some()));
        }

        let short_names = shorten_branch_names(&base_names);
        let mut branch_names = Namer::with_reserved(&["Self"]);
        let out = tys
            .into_iter()
            .zip(short_names)
            .map(|(ty, short)| Branch { name: branch_names.unique(&short), ty })
            .collect();

        // `std::variant` index. Mojang documents it as `uint32`; the binary
        // stream writes it as a varint. Branches that start with a `const`
        // field still get the index: the constant is written after it.
        let int = node.control.and_then(int_of).unwrap_or(Int::U32);
        let doc = join_doc(std::slice::from_ref(&node.description));
        Ok(VariantDef { name, source: source.to_owned(), doc, index: (int, Enc::Var), branches: out })
    }
}

/// The base type behind aliases and constraints.
fn base_type(ty: &Ty) -> &Ty {
    match ty {
        Ty::Alias { target, .. } => base_type(target),
        Ty::Constrained(inner, _) => base_type(inner),
        other => other,
    }
}

/// The documented limits of `node` that are meaningful for `ty`.
fn constraints_for(ty: &Ty, node: &Node) -> Constraints {
    let mut constraints = Constraints::default();
    match base_type(ty) {
        Ty::Int(int, _) => {
            // Bounds the integer type already guarantees carry no information.
            let (type_min, type_max) = int.range();
            constraints.minimum = node.minimum.filter(|min| *min > type_min as f64).map(|min| format!("{min:?}"));
            constraints.maximum = node.maximum.filter(|max| *max < type_max as f64).map(|max| format!("{max:?}"));
        }
        Ty::F32 | Ty::F64 => {
            constraints.minimum = node.minimum.map(|min| format!("{min:?}"));
            constraints.maximum = node.maximum.map(|max| format!("{max:?}"));
        }
        Ty::String => {
            constraints.min_length = node.min_length.filter(|min| *min > 0);
            constraints.max_length = node.max_length;
        }
        Ty::Vec(..) => {
            constraints.min_items = node.min_items.filter(|min| *min > 0);
            // A couple of arrays carry `maxProperties` instead of `maxItems`.
            constraints.max_items = node.max_items.or(node.max_properties);
        }
        Ty::Map(..) => constraints.max_items = node.max_properties,
        // Fixed-size arrays have their length in the type; nothing else has
        // limits in the schemas.
        _ => {}
    }
    constraints
}

/// Strips the words shared by every referenced branch name
/// (`StartVideoCapture` / `StopVideoCapture` -> `Start` / `Stop`).
fn shorten_branch_names(names: &[(String, bool)]) -> Vec<String> {
    let referenced: Vec<Vec<&str>> =
        names.iter().filter(|(_, is_ref)| *is_ref).map(|(name, _)| names::pascal_words(name)).collect();
    if referenced.len() < 2 {
        return names.iter().map(|(name, _)| name.clone()).collect();
    }
    let shortest = referenced.iter().map(Vec::len).min().unwrap_or(0);
    let mut prefix = 0;
    while prefix < shortest && referenced.iter().all(|words| words[prefix] == referenced[0][prefix]) {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < shortest
        && referenced.iter().all(|words| words[words.len() - 1 - suffix] == referenced[0][referenced[0].len() - 1 - suffix])
    {
        suffix += 1;
    }
    // Never strip a name down to nothing.
    while prefix + suffix >= shortest && (prefix > 0 || suffix > 0) {
        if suffix > 0 {
            suffix -= 1;
        } else {
            prefix -= 1;
        }
    }
    names
        .iter()
        .map(|(name, is_ref)| {
            if !*is_ref {
                return name.clone();
            }
            let words = names::pascal_words(name);
            let short = words[prefix..words.len() - suffix].concat();
            // A shortened name must still be a valid identifier.
            if short.starts_with(|c: char| c.is_ascii_alphabetic()) {
                short
            } else {
                name.clone()
            }
        })
        .collect()
}

fn build_enum(name: String, source: &str, node: &Node, location: &str) -> Result<EnumDef> {
    let enumerators = node.enumerators.as_ref().ok_or_else(|| format!("{location}: enum without enumerators"))?;
    let mut repr = enum_int(node.underlying);
    if enumerators.iter().any(|(_, value)| !repr.fits(*value)) {
        // The documented underlying type cannot hold every value.
        repr = Int::I64;
    }

    // Variants and alias constants share the `Enum::Name` namespace.
    let mut idents = Namer::with_reserved(&["Self"]);
    let mut by_value: HashMap<i64, String> = HashMap::new();
    let mut variants = Vec::new();
    let mut aliases = Vec::new();
    for (original, value) in enumerators {
        let ident = idents.unique(&names::pascal_case(original));
        let doc = node.enum_descriptions.get(original).cloned();
        match by_value.get(value) {
            // C++ enumerators may share a value; Rust discriminants may not.
            // The first one becomes the variant, the others become constants.
            Some(target) => {
                aliases.push(EnumAlias { name: ident, original: original.clone(), target: target.clone(), doc })
            }
            None => {
                by_value.insert(*value, ident.clone());
                variants.push(EnumVariant { name: ident, original: original.clone(), value: *value, doc });
            }
        }
    }
    Ok(EnumDef { name, source: source.to_owned(), doc: node.description.clone(), repr, variants, aliases })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::parse_document;
    use serde_json::{json, Value};

    fn docs(entries: &[(&str, Value)]) -> BTreeMap<String, Document> {
        entries
            .iter()
            .map(|(stem, body)| {
                let mut base = json!({
                    "title": stem, "$schema": "s", "$id": "i", "x-format-version": "v",
                    "x-minecraft-version": "1.0", "x-protocol-version": 1
                });
                base.as_object_mut().unwrap().extend(body.as_object().unwrap().clone());
                ((*stem).to_owned(), parse_document(stem, &base).unwrap())
            })
            .collect()
    }

    #[test]
    fn duplicate_enum_values_become_aliases() {
        let docs = docs(&[(
            "Mode",
            json!({"type": "string", "enum": ["A", "B", "AliasOfA", "self", "3d"],
                   "x-underlying-type": "uint8", "x-enum-binary-value": [0, 1, 0, 2, 300]}),
        )]);
        let model = Lowerer::new(&docs).run().unwrap();
        let def = &model.enums[0];
        assert_eq!(def.variants.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), ["A", "B", "Self_", "_3d"]);
        assert_eq!(def.aliases.len(), 1);
        assert_eq!((def.aliases[0].name.as_str(), def.aliases[0].target.as_str()), ("AliasOfA", "A"));
        // 300 does not fit in the documented uint8.
        assert_eq!(def.repr, Int::I64);

    }

    #[test]
    fn struct_fields_follow_ordinals_and_optionality() {
        let docs = docs(&[(
            "S",
            json!({"type": "object", "required": ["Type", "List"], "properties": {
                "Maybe": {"type": "string", "x-ordinal-index": 2},
                "Defaulted": {"type": "array", "items": {"type": "boolean"}, "default": [], "x-ordinal-index": 3},
                "List": {"type": "array", "items": {"type": "integer", "x-underlying-type": "uint64"},
                         "x-serialization-options": ["Compression", "No size compression"], "x-ordinal-index": 1},
                "Type": {"type": "integer", "x-underlying-type": "int32",
                         "x-serialization-options": ["Big Endian"], "x-ordinal-index": 0}
            }}),
        )]);
        let model = Lowerer::new(&docs).run().unwrap();
        let fields = &model.structs[0].fields;
        assert_eq!(
            fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["r#type", "list", "maybe", "defaulted"]
        );
        // Not required but documented with a default: always on the wire.
        assert_eq!(fields[3].ty, Ty::Vec(Box::new(Ty::Bool), LenEnc::Var));
        assert_eq!(fields[0].ty, Ty::Int(Int::I32, Enc::Be));
        assert_eq!(fields[1].ty, Ty::Vec(Box::new(Ty::Int(Int::U64, Enc::Var)), LenEnc::U32Le));
        assert_eq!(fields[2].ty, Ty::Option(Box::new(Ty::String)));
    }

    #[test]
    fn tagged_and_indexed_variants() {
        let docs = docs(&[
            ("Kind", json!({"type": "string", "enum": ["Byte", "ShortValue"], "x-underlying-type": "uint8",
                            "x-enum-binary-value": [0, 1]})),
            ("BytePayload", json!({"type": "object", "required": ["Type", "V"], "properties": {
                "Type": {"const": "byte", "$ref": "./Kind.json", "x-underlying-type": "uint8",
                         "x-serialization-options": ["Enum-as-Value"], "x-ordinal-index": 0},
                "V": {"type": "integer", "x-underlying-type": "int8", "x-ordinal-index": 1}}})),
            ("ShortPayload", json!({"type": "object", "required": ["Type"], "properties": {
                "Type": {"const": "shortvalue", "$ref": "./Kind.json", "x-underlying-type": "uint8",
                         "x-serialization-options": ["Enum-as-Value"], "x-ordinal-index": 0}}})),
            ("Holder", json!({"type": "object", "required": ["Tagged", "Indexed"], "properties": {
                "Tagged": {"oneOf": [{"$ref": "./BytePayload.json"}, {"$ref": "./ShortPayload.json"}],
                           "x-control-value-type": "uint32", "x-ordinal-index": 0},
                "Indexed": {"oneOf": [{"type": "null"}, {"type": "boolean"}, {"$ref": "./BytePayload.json"}],
                            "x-control-value-type": "uint32", "x-ordinal-index": 1}}})),
        ]);
        let model = Lowerer::new(&docs).run().unwrap();
        // Branches led by a constant are indexed like any other variant: the
        // constant is a field of the branch, written after the index.
        let tagged = model.variants.iter().find(|v| v.name == "HolderTagged").unwrap();
        assert_eq!(tagged.index, (Int::U32, Enc::Var));
        assert_eq!(tagged.branches.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["Byte", "Short"]);
        let indexed = model.variants.iter().find(|v| v.name == "HolderIndexed").unwrap();
        assert_eq!(indexed.index, (Int::U32, Enc::Var));
        assert_eq!(indexed.branches.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["Null", "Bool", "BytePayload"]);
        assert_eq!(indexed.branches[0].ty, Ty::Unit);
        let payload = model.structs.iter().find(|s| s.name == "BytePayload").unwrap();
        assert_eq!(payload.fields[0].constant.as_ref().unwrap().variant, "Byte");
    }

    #[test]
    fn aliases_maps_and_self_references() {
        let docs = docs(&[
            ("Token", json!({"$ref": "./Token.json"})),
            ("Id", json!({"type": "integer"})),
            ("Name", json!({"type": "string"})),
            ("S", json!({"type": "object", "required": ["T", "I", "M", "N", "Fixed", "Nbt"], "properties": {
                "T": {"$ref": "./Token.json", "x-ordinal-index": 0},
                "I": {"$ref": "./Id.json", "x-ordinal-index": 1},
                "M": {"type": "object", "x-ordinal-index": 2, "additionalProperties": {"type": "object",
                      "properties": {"key": {"$ref": "./Name.json"}, "value": {"type": "boolean"}}}},
                "N": {"type": "object", "x-ordinal-index": 3,
                      "additionalProperties": {"type": "number", "x-underlying-type": "float"}},
                "Fixed": {"type": "array", "items": {"type": "number", "x-underlying-type": "float"},
                          "minItems": 3, "maxItems": 3, "x-ordinal-index": 4},
                "Nbt": {"x-ordinal-index": 5}}})),
        ]);
        let model = Lowerer::new(&docs).run().unwrap();
        assert_eq!(model.aliases.iter().find(|a| a.name == "Token").unwrap().ty, Ty::String);
        let fields = &model.structs[0].fields;
        assert_eq!(fields[0].ty, Ty::Alias { name: "Token".into(), target: Box::new(Ty::String) });
        assert_eq!(fields[1].ty, Ty::Alias { name: "Id".into(), target: Box::new(Ty::Int(Int::I32, Enc::Var)) });
        assert_eq!(
            fields[2].ty,
            Ty::Map(Box::new(Ty::Alias { name: "Name".into(), target: Box::new(Ty::String) }), Box::new(Ty::Bool))
        );
        assert_eq!(fields[3].ty, Ty::Map(Box::new(Ty::String), Box::new(Ty::F32)));
        assert_eq!(fields[4].ty, Ty::Array(Box::new(Ty::F32), 3));
        assert_eq!(fields[5].ty, Ty::Nbt);
    }

    #[test]
    fn packets_are_sorted_and_ids_checked() {
        let packet = |id: u32| json!({"$ref": "./P.json", "$metaProperties": {"[cereal:packet]": id}});
        let ok = docs(&[("P", json!({"type": "object"})), ("BPacket", packet(2)), ("APacket", packet(9))]);
        let model = Lowerer::new(&ok).run().unwrap();
        assert_eq!(model.packets.iter().map(|p| (p.variant.as_str(), p.id)).collect::<Vec<_>>(), [("B", 2), ("A", 9)]);
        let clash = docs(&[("P", json!({"type": "object"})), ("BPacket", packet(2)), ("APacket", packet(2))]);
        assert!(Lowerer::new(&clash).run().is_err());
        let dangling = docs(&[("APacket", packet(1))]);
        assert!(Lowerer::new(&dangling).run().is_err());
    }

    #[test]
    fn a_type_embedding_a_packet_embeds_its_payload() {
        let docs = docs(&[
            ("P", json!({"type": "object"})),
            ("APacket", json!({"$ref": "./P.json", "$metaProperties": {"[cereal:packet]": 1}})),
            ("S", json!({"type": "object", "required": ["Inner"], "properties": {
                "Inner": {"$ref": "./APacket.json", "x-ordinal-index": 0}}})),
        ]);
        let model = Lowerer::new(&docs).run().unwrap();
        let holder = model.structs.iter().find(|s| s.name == "S").unwrap();
        assert_eq!(holder.fields[0].ty, Ty::Named("P".into()));
    }

    #[test]
    fn ambiguous_schemas_are_rejected() {
        let error = |entries: &[(&str, Value)]| Lowerer::new(&docs(entries)).run().unwrap_err();

        // Two properties claiming the same wire position.
        let message = error(&[("S", json!({"type": "object", "properties": {
            "A": {"type": "boolean", "x-ordinal-index": 0},
            "B": {"type": "boolean", "x-ordinal-index": 0}}}))]);
        assert!(message.contains("share x-ordinal-index 0"), "{message}");

        let kind = json!({"type": "string", "enum": ["Item_Tag", "ItemTag", "Other"],
                          "x-underlying-type": "uint8", "x-enum-binary-value": [0, 1, 2]});
        let tagged = |constant: &str| json!({"type": "object", "required": ["Type"], "properties": {
            "Type": {"const": constant, "$ref": "./Kind.json", "x-underlying-type": "uint8",
                     "x-serialization-options": ["Enum-as-Value"], "x-ordinal-index": 0}}});

        // A constant matching two different enumerators.
        let message = error(&[("Kind", kind), ("A", tagged("itemtag"))]);
        assert!(message.contains("ambiguous"), "{message}");
    }

    #[test]
    fn constraints_are_kept_when_meaningful() {
        let docs = docs(&[(
            "S",
            json!({"type": "object", "required": ["Name", "Byte", "Level", "Items"], "properties": {
                "Name": {"type": "string", "minLength": 1, "maxLength": 16, "x-ordinal-index": 0},
                // 0..=255 on a uint8 says nothing the type does not.
                "Byte": {"type": "integer", "x-underlying-type": "uint8", "minimum": 0.0, "maximum": 255.0,
                         "x-ordinal-index": 1},
                "Level": {"type": "integer", "x-underlying-type": "int32", "minimum": 0.0, "maximum": 100.0,
                          "x-ordinal-index": 2},
                "Items": {"type": "array", "items": {"type": "boolean"}, "minItems": 1, "maxItems": 4,
                          "x-ordinal-index": 3}}}),
        )]);
        let model = Lowerer::new(&docs).run().unwrap();
        let fields = &model.structs[0].fields;
        let Ty::Constrained(inner, limits) = &fields[0].ty else { panic!("{:?}", fields[0].ty) };
        assert_eq!(**inner, Ty::String);
        assert_eq!((limits.min_length, limits.max_length), (Some(1), Some(16)));
        assert_eq!(fields[1].ty, Ty::Int(Int::U8, Enc::Le));
        let Ty::Constrained(_, limits) = &fields[2].ty else { panic!("{:?}", fields[2].ty) };
        assert_eq!((limits.minimum.as_deref(), limits.maximum.as_deref()), (Some("0.0"), Some("100.0")));
        assert_eq!(limits.describe(), "minimum 0, maximum 100");
        let Ty::Constrained(_, limits) = &fields[3].ty else { panic!("{:?}", fields[3].ty) };
        assert_eq!((limits.min_items, limits.max_items), (Some(1), Some(4)));
    }

    #[test]
    fn overrides_replace_plain_strings_only() {
        let payload = |field: Value| json!({"type": "object", "required": ["Connection Request"],
            "properties": {"Connection Request": field}});
        let ok = docs(&[("LoginPacketPayload", payload(json!({"type": "string", "x-ordinal-index": 0})))]);
        let model = Lowerer::new(&ok).run().unwrap();
        assert_eq!(model.structs[0].fields[0].ty, Ty::Extern("ConnectionRequest".into()));

        // The property changed shape: the override must be reviewed.
        let changed = docs(&[("LoginPacketPayload", payload(json!({"type": "integer", "x-ordinal-index": 0})))]);
        assert!(Lowerer::new(&changed).run().unwrap_err().contains("no longer a plain string"));

        // The property disappeared.
        let gone = docs(&[("LoginPacketPayload", json!({"type": "object"}))]);
        assert!(Lowerer::new(&gone).run().unwrap_err().contains("matches no property"));
    }
}
