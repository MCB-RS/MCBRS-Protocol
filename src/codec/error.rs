//! Error type shared by every encoder / decoder of the crate.

use std::fmt;

/// Convenience alias used throughout the crate.
pub type Result<T, E = ProtocolError> = std::result::Result<T, E>;

/// Everything that can go wrong while encoding or decoding protocol data.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProtocolError {
    /// The buffer ended before the value was fully read.
    UnexpectedEof { needed: usize, remaining: usize },
    /// A variable-length integer used more bytes than its type allows.
    VarIntTooLong { max_bytes: usize },
    /// A decoded integer does not fit in the destination type.
    IntegerOutOfRange { type_name: &'static str, value: i128 },
    /// A length prefix is too large to be encoded or is not plausible.
    LengthOutOfRange { length: u64 },
    /// A string is not valid UTF-8.
    InvalidUtf8,
    /// A numeric value does not map to any variant of the enum.
    InvalidEnumValue { enum_name: &'static str, value: i64 },
    /// A textual value does not map to any variant of the enum.
    InvalidEnumName { enum_name: &'static str, name: String },
    /// The discriminant of a variant (`oneOf`) type is unknown.
    InvalidVariantIndex { type_name: &'static str, index: i64 },
    /// A constant discriminator field did not hold the expected value.
    UnexpectedTag { type_name: &'static str, expected: i64, found: i64 },
    /// No packet is registered for this identifier.
    UnknownPacketId(u32),
    /// An NBT tag identifier is not part of the format.
    InvalidNbtTagType(u8),
    /// NBT data is nested deeper than [`crate::codec::nbt::MAX_DEPTH`].
    NbtTooDeep,
    /// Bytes were left in the buffer after a complete value was decoded.
    TrailingBytes { remaining: usize },
    /// A packet could not be encoded or decoded; `source` says why.
    Packet { name: &'static str, source: Box<ProtocolError> },
}

impl ProtocolError {
    /// Attaches the name of the packet being processed to the error.
    pub fn in_packet(self, name: &'static str) -> Self {
        match self {
            // Already attributed (nested call): keep the innermost packet.
            Self::Packet { .. } => self,
            source => Self::Packet { name, source: Box::new(source) },
        }
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof { needed, remaining } => {
                write!(f, "unexpected end of buffer: needed {needed} byte(s), {remaining} remaining")
            }
            Self::VarIntTooLong { max_bytes } => write!(f, "varint longer than {max_bytes} bytes"),
            Self::IntegerOutOfRange { type_name, value } => write!(f, "value {value} does not fit in {type_name}"),
            Self::LengthOutOfRange { length } => write!(f, "length {length} is out of range"),
            Self::InvalidUtf8 => f.write_str("string is not valid UTF-8"),
            Self::InvalidEnumValue { enum_name, value } => write!(f, "invalid value {value} for enum {enum_name}"),
            Self::InvalidEnumName { enum_name, name } => write!(f, "invalid name {name:?} for enum {enum_name}"),
            Self::InvalidVariantIndex { type_name, index } => {
                write!(f, "invalid variant index {index} for {type_name}")
            }
            Self::UnexpectedTag { type_name, expected, found } => {
                write!(f, "unexpected discriminator for {type_name}: expected {expected}, found {found}")
            }
            Self::UnknownPacketId(id) => write!(f, "unknown packet id {id}"),
            Self::InvalidNbtTagType(id) => write!(f, "invalid NBT tag type {id}"),
            Self::NbtTooDeep => f.write_str("NBT data is nested too deeply"),
            Self::TrailingBytes { remaining } => write!(f, "{remaining} trailing byte(s) after value"),
            Self::Packet { name, source } => write!(f, "{name}: {source}"),
        }
    }
}

impl std::error::Error for ProtocolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Packet { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}
