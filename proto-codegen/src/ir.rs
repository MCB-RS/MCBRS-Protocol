//! Intermediate representation: what will be emitted as Rust, independent of
//! both the JSON input and the textual output.

/// Integer type, both as a Rust type and as a wire width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Int {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
}

impl Int {
    pub fn rust(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::I8 => "i8",
            Self::U16 => "u16",
            Self::I16 => "i16",
            Self::U32 => "u32",
            Self::I32 => "i32",
            Self::U64 => "u64",
            Self::I64 => "i64",
        }
    }

    pub fn range(self) -> (i128, i128) {
        match self {
            Self::U8 => (0, u8::MAX as i128),
            Self::I8 => (i8::MIN as i128, i8::MAX as i128),
            Self::U16 => (0, u16::MAX as i128),
            Self::I16 => (i16::MIN as i128, i16::MAX as i128),
            Self::U32 => (0, u32::MAX as i128),
            Self::I32 => (i32::MIN as i128, i32::MAX as i128),
            Self::U64 => (0, u64::MAX as i128),
            Self::I64 => (i64::MIN as i128, i64::MAX as i128),
        }
    }

    pub fn fits(self, value: i64) -> bool {
        let (min, max) = self.range();
        (min..=max).contains(&(value as i128))
    }
}

/// How an integer is laid out on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Enc {
    /// Fixed width, little-endian (the default).
    Le,
    /// Fixed width, big-endian (`"Big Endian"`).
    Be,
    /// Varint (`"Compression"`): LEB128, ZigZag first for signed types.
    Var,
}

/// Name of the `codec::put_*` / `codec::get_*` pair for an integer.
pub fn int_fn_suffix(int: Int, enc: Enc) -> String {
    match (int, enc) {
        (_, Enc::Var) => format!("{}_var", int.rust()),
        (Int::U8 | Int::I8, _) => int.rust().to_owned(),
        (_, Enc::Le) => format!("{}_le", int.rust()),
        (_, Enc::Be) => format!("{}_be", int.rust()),
    }
}

/// Length prefix of a variable-size collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LenEnc {
    /// `VarUInt32` (the default).
    Var,
    /// Fixed little-endian `u32` (`"No size compression"`).
    U32Le,
}

/// How an enum value is laid out on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnumWire {
    /// As its number (`"Enum-as-Value"`).
    Raw(Int, Enc),
    /// As its name, in a string.
    Name,
}

/// A type as used by a field: Rust type plus wire encoding.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    Bool,
    Int(Int, Enc),
    F32,
    F64,
    String,
    /// Untyped field in the schema: network NBT.
    Nbt,
    /// `null` / empty object: nothing on the wire.
    Unit,
    /// A generated struct or variant enum, which implements `BedrockCodec`.
    Named(String),
    /// A generated protocol enum, with its use-site encoding.
    Enum { name: String, wire: EnumWire, allow_unknown: bool },
    /// A generated `type` alias; encoded like its target.
    Alias { name: String, target: Box<Ty> },
    /// `std::optional<T>`: bool flag + value.
    Option(Box<Ty>),
    /// `std::vector<T>`: length prefix + elements.
    Vec(Box<Ty>, LenEnc),
    /// Fixed element count, no prefix, small enough for a Rust array.
    Array(Box<Ty>, usize),
    /// Fixed element count, no prefix, kept on the heap.
    FixedVec(Box<Ty>, usize),
    /// Associative container: `VarUInt32` count + (key, value) pairs.
    Map(Box<Ty>, Box<Ty>),
    /// Indirection inserted to break a recursive type.
    Boxed(Box<Ty>),
}

#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: String,
    /// Name in Mojang's documentation.
    pub original: String,
    pub value: i64,
    pub doc: Option<String>,
}

/// A C++ enumerator sharing its value with an earlier one.
#[derive(Debug, Clone)]
pub struct EnumAlias {
    pub name: String,
    pub original: String,
    /// Rust name of the variant it aliases.
    pub target: String,
    pub doc: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub source: String,
    pub doc: Option<String>,
    /// Integer type of `value()`.
    pub repr: Int,
    /// Variants with pairwise distinct values.
    pub variants: Vec<EnumVariant>,
    pub aliases: Vec<EnumAlias>,
}

/// A field whose value is fixed by the schema (`const`): it is written and
/// checked on the wire but not stored in the struct.
#[derive(Debug, Clone)]
pub struct ConstValue {
    pub enum_name: String,
    pub variant: String,
    /// Name of the associated constant on the struct.
    pub const_name: String,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub original: String,
    pub doc: Option<String>,
    pub ty: Ty,
    pub constant: Option<ConstValue>,
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub source: String,
    pub doc: Option<String>,
    /// In wire order.
    pub fields: Vec<Field>,
}

impl StructDef {
    /// The leading constant field, if any: it discriminates the struct when
    /// used as a branch of a tagged variant.
    pub fn tag(&self) -> Option<&Field> {
        self.fields.first().filter(|field| field.constant.is_some())
    }
}

#[derive(Debug, Clone)]
pub struct Branch {
    pub name: String,
    pub ty: Ty,
    /// For tagged variants: the enum variant selecting this branch.
    pub tag_variant: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariantKind {
    /// `std::variant`: the branch index, then the branch.
    Index(Int, Enc),
    /// Every branch is a struct starting with a constant field of the same
    /// enum: that field is the discriminator, no index is written.
    Tagged { tag: Ty },
}

#[derive(Debug, Clone)]
pub struct VariantDef {
    pub name: String,
    pub source: String,
    pub doc: Option<String>,
    pub kind: VariantKind,
    pub branches: Vec<Branch>,
}

#[derive(Debug, Clone)]
pub struct AliasDef {
    pub name: String,
    pub source: String,
    pub doc: Option<String>,
    pub ty: Ty,
}

#[derive(Debug, Clone)]
pub struct PacketDef {
    pub name: String,
    /// Name of the variant in the global `Packet` enum.
    pub variant: String,
    pub source: String,
    pub id: u32,
    pub doc: Option<String>,
    pub payload: Ty,
}

/// Everything the emitter needs.
#[derive(Debug, Default)]
pub struct Model {
    pub enums: Vec<EnumDef>,
    pub structs: Vec<StructDef>,
    pub variants: Vec<VariantDef>,
    pub aliases: Vec<AliasDef>,
    /// Sorted by packet id.
    pub packets: Vec<PacketDef>,
    pub minecraft_version: String,
    pub protocol_version: i64,
}
