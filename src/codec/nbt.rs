//! Named Binary Tag support.
//!
//! Bedrock uses two flavours of NBT:
//!
//! * [`Nbt`]: the little-endian disk format (fixed-width integers, `u16` string
//!   lengths), also used by a few packets;
//! * [`NetworkNbt`]: the network format (ZigZag varints for `Int`, `Long` and
//!   lengths, `VarUInt32` string lengths). This is what Mojang's schemas mean
//!   by an untyped field, and what the generated code uses.

use super::error::{ProtocolError, Result};
use super::io::*;
use super::BedrockCodec;
use bytes::{Buf, BufMut};

/// Maximum nesting depth accepted while decoding.
pub const MAX_DEPTH: usize = 512;

/// A single NBT value.
#[derive(Debug, Clone, PartialEq)]
pub enum NbtTag {
    /// `TAG_End`: only valid as a root (meaning "no data") or as the element
    /// type of an empty list.
    End,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    /// Homogeneous list; the element type is taken from the first element.
    List(Vec<NbtTag>),
    /// Named children, in wire order.
    Compound(Vec<(String, NbtTag)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl NbtTag {
    /// Numeric tag type identifier.
    pub const fn type_id(&self) -> u8 {
        match self {
            Self::End => 0,
            Self::Byte(_) => 1,
            Self::Short(_) => 2,
            Self::Int(_) => 3,
            Self::Long(_) => 4,
            Self::Float(_) => 5,
            Self::Double(_) => 6,
            Self::ByteArray(_) => 7,
            Self::String(_) => 8,
            Self::List(_) => 9,
            Self::Compound(_) => 10,
            Self::IntArray(_) => 11,
            Self::LongArray(_) => 12,
        }
    }

    /// Child of a compound by name.
    pub fn get(&self, key: &str) -> Option<&NbtTag> {
        match self {
            Self::Compound(children) => children.iter().find(|(name, _)| name == key).map(|(_, tag)| tag),
            _ => None,
        }
    }
}

impl Default for NbtTag {
    fn default() -> Self {
        Self::Compound(Vec::new())
    }
}

/// Container being decoded.
enum Frame {
    List { element_type: u8, remaining: usize, items: Vec<NbtTag> },
    Compound { children: Vec<(String, NbtTag)>, pending: Option<String> },
}

enum Started {
    Value(NbtTag),
    Frame(Frame),
}

/// Integer / length encoding of an NBT stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NbtEncoding {
    LittleEndian,
    Network,
}

impl NbtEncoding {
    fn put_int<B: BufMut>(self, buf: &mut B, value: i32) {
        match self {
            Self::LittleEndian => put_i32_le(buf, value),
            Self::Network => put_i32_var(buf, value),
        }
    }

    fn get_int<B: Buf>(self, buf: &mut B) -> Result<i32> {
        match self {
            Self::LittleEndian => get_i32_le(buf),
            Self::Network => get_i32_var(buf),
        }
    }

    fn put_long<B: BufMut>(self, buf: &mut B, value: i64) {
        match self {
            Self::LittleEndian => put_i64_le(buf, value),
            Self::Network => put_i64_var(buf, value),
        }
    }

    fn get_long<B: Buf>(self, buf: &mut B) -> Result<i64> {
        match self {
            Self::LittleEndian => get_i64_le(buf),
            Self::Network => get_i64_var(buf),
        }
    }

    fn put_len<B: BufMut>(self, buf: &mut B, len: usize) -> Result<()> {
        let len = i32::try_from(len).map_err(|_| ProtocolError::LengthOutOfRange { length: len as u64 })?;
        self.put_int(buf, len);
        Ok(())
    }

    fn get_len<B: Buf>(self, buf: &mut B) -> Result<usize> {
        let len = self.get_int(buf)?;
        usize::try_from(len).map_err(|_| ProtocolError::LengthOutOfRange { length: len as u32 as u64 })
    }

    fn put_str<B: BufMut>(self, buf: &mut B, value: &str) -> Result<()> {
        match self {
            Self::LittleEndian => {
                let len = u16::try_from(value.len())
                    .map_err(|_| ProtocolError::LengthOutOfRange { length: value.len() as u64 })?;
                put_u16_le(buf, len);
                buf.put_slice(value.as_bytes());
                Ok(())
            }
            Self::Network => put_string(buf, value),
        }
    }

    fn get_str<B: Buf>(self, buf: &mut B) -> Result<String> {
        match self {
            Self::LittleEndian => {
                let len = get_u16_le(buf)? as usize;
                String::from_utf8(get_bytes(buf, len)?).map_err(|_| ProtocolError::InvalidUtf8)
            }
            Self::Network => get_string(buf),
        }
    }

    /// Writes the payload of `tag`. `depth` is the number of containers
    /// around it: the recursion is bounded by [`MAX_DEPTH`], which both keeps
    /// the stack use small and guarantees the output can be decoded back.
    fn put_payload<B: BufMut>(self, buf: &mut B, tag: &NbtTag, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(ProtocolError::NbtTooDeep);
        }
        match tag {
            NbtTag::End => {}
            NbtTag::Byte(v) => put_i8(buf, *v),
            NbtTag::Short(v) => put_i16_le(buf, *v),
            NbtTag::Int(v) => self.put_int(buf, *v),
            NbtTag::Long(v) => self.put_long(buf, *v),
            NbtTag::Float(v) => put_f32_le(buf, *v),
            NbtTag::Double(v) => put_f64_le(buf, *v),
            NbtTag::ByteArray(bytes) => {
                self.put_len(buf, bytes.len())?;
                buf.put_slice(bytes);
            }
            NbtTag::String(v) => self.put_str(buf, v)?,
            NbtTag::List(items) => {
                let element_type = items.first().map_or(0, NbtTag::type_id);
                // `End` has no payload and doubles as the compound terminator:
                // it cannot be an element. Lists are homogeneous.
                if let Some(bad) = items.iter().find(|item| item.type_id() != element_type || element_type == 0) {
                    return Err(ProtocolError::InvalidNbtTagType(bad.type_id()));
                }
                put_u8(buf, element_type);
                self.put_len(buf, items.len())?;
                for item in items {
                    self.put_payload(buf, item, depth + 1)?;
                }
            }
            NbtTag::Compound(children) => {
                for (name, child) in children {
                    if matches!(child, NbtTag::End) {
                        // Would be read back as the end of the compound.
                        return Err(ProtocolError::InvalidNbtTagType(0));
                    }
                    put_u8(buf, child.type_id());
                    self.put_str(buf, name)?;
                    self.put_payload(buf, child, depth + 1)?;
                }
                put_u8(buf, 0);
            }
            NbtTag::IntArray(values) => {
                self.put_len(buf, values.len())?;
                for v in values {
                    self.put_int(buf, *v);
                }
            }
            NbtTag::LongArray(values) => {
                self.put_len(buf, values.len())?;
                for v in values {
                    self.put_long(buf, *v);
                }
            }
        }
        Ok(())
    }

    /// Reads a scalar tag, or opens a container frame for lists and compounds.
    fn start_value<B: Buf>(self, buf: &mut B, type_id: u8) -> Result<Started> {
        Ok(Started::Value(match type_id {
            0 => NbtTag::End,
            1 => NbtTag::Byte(get_i8(buf)?),
            2 => NbtTag::Short(get_i16_le(buf)?),
            3 => NbtTag::Int(self.get_int(buf)?),
            4 => NbtTag::Long(self.get_long(buf)?),
            5 => NbtTag::Float(get_f32_le(buf)?),
            6 => NbtTag::Double(get_f64_le(buf)?),
            7 => {
                let len = self.get_len(buf)?;
                NbtTag::ByteArray(get_bytes(buf, len)?)
            }
            8 => NbtTag::String(self.get_str(buf)?),
            9 => {
                let element_type = get_u8(buf)?;
                let remaining = self.get_len(buf)?;
                if element_type > 12 || (element_type == 0 && remaining != 0) {
                    return Err(ProtocolError::InvalidNbtTagType(element_type));
                }
                // The count is untrusted: reserve a bounded amount, the
                // vector grows as elements are really decoded.
                let items = Vec::with_capacity(bounded_capacity::<NbtTag>(remaining, buf.remaining()));
                return Ok(Started::Frame(Frame::List { element_type, remaining, items }));
            }
            10 => return Ok(Started::Frame(Frame::Compound { children: Vec::new(), pending: None })),
            11 => {
                let len = self.get_len(buf)?;
                NbtTag::IntArray(get_vec(buf, len, |buf| self.get_int(buf))?)
            }
            12 => {
                let len = self.get_len(buf)?;
                NbtTag::LongArray(get_vec(buf, len, |buf| self.get_long(buf))?)
            }
            other => return Err(ProtocolError::InvalidNbtTagType(other)),
        }))
    }

    /// Reads the payload of a tag of type `type_id`.
    ///
    /// Containers are tracked on an explicit stack rather than through
    /// recursion, so hostile nesting can never overflow the thread stack.
    fn get_payload<B: Buf>(self, buf: &mut B, type_id: u8) -> Result<NbtTag> {
        let mut stack: Vec<Frame> = Vec::new();
        let mut next_type = type_id;
        loop {
            // Read one value; containers are pushed and filled by later turns.
            let mut done = match self.start_value(buf, next_type)? {
                Started::Value(value) => Some(value),
                Started::Frame(frame) => {
                    if stack.len() >= MAX_DEPTH {
                        return Err(ProtocolError::NbtTooDeep);
                    }
                    stack.push(frame);
                    None
                }
            };
            // Attach finished values to their parent and close the containers
            // that are complete, until one of them needs another element.
            next_type = loop {
                let Some(top) = stack.last_mut() else {
                    // No container left: `done` is the root value.
                    return done.ok_or(ProtocolError::InvalidNbtTagType(type_id));
                };
                if let Some(value) = done.take() {
                    match top {
                        Frame::List { items, .. } => items.push(value),
                        Frame::Compound { children, pending } => {
                            children.push((pending.take().unwrap_or_default(), value))
                        }
                    }
                }
                match top {
                    Frame::List { element_type, remaining, .. } if *remaining > 0 => {
                        *remaining -= 1;
                        break *element_type;
                    }
                    Frame::List { .. } => {}
                    Frame::Compound { pending, .. } => {
                        let child_type = get_u8(buf)?;
                        if child_type != 0 {
                            *pending = Some(self.get_str(buf)?);
                            break child_type;
                        }
                    }
                }
                // The top container is complete: it becomes a finished value.
                done = stack.pop().map(|frame| match frame {
                    Frame::List { items, .. } => NbtTag::List(items),
                    Frame::Compound { children, .. } => NbtTag::Compound(children),
                });
            };
        }
    }

    /// Writes a root tag: type id, name, payload (a lone `0` byte for `End`).
    pub fn write_root<B: BufMut>(self, buf: &mut B, name: &str, tag: &NbtTag) -> Result<()> {
        put_u8(buf, tag.type_id());
        if matches!(tag, NbtTag::End) {
            return Ok(());
        }
        self.put_str(buf, name)?;
        self.put_payload(buf, tag, 0)
    }

    /// Reads a root tag and its name.
    pub fn read_root<B: Buf>(self, buf: &mut B) -> Result<(String, NbtTag)> {
        let type_id = get_u8(buf)?;
        if type_id == 0 {
            return Ok((String::new(), NbtTag::End));
        }
        let name = self.get_str(buf)?;
        Ok((name, self.get_payload(buf, type_id)?))
    }
}

macro_rules! nbt_root {
    ($(#[$doc:meta])* $name:ident, $encoding:expr) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Default)]
        pub struct $name {
            /// Name of the root tag (almost always empty).
            pub name: String,
            pub root: NbtTag,
        }

        impl $name {
            pub const ENCODING: NbtEncoding = $encoding;

            pub fn new(root: NbtTag) -> Self {
                Self { name: String::new(), root }
            }
        }

        impl From<NbtTag> for $name {
            fn from(root: NbtTag) -> Self {
                Self::new(root)
            }
        }

        impl BedrockCodec for $name {
            fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()> {
                Self::ENCODING.write_root(buf, &self.name, &self.root)
            }
            fn deserialize<B: Buf>(buf: &mut B) -> Result<Self> {
                let (name, root) = Self::ENCODING.read_root(buf)?;
                Ok(Self { name, root })
            }
        }
    };
}

nbt_root!(
    /// Root NBT tag in the little-endian format.
    Nbt,
    NbtEncoding::LittleEndian
);
nbt_root!(
    /// Root NBT tag in the network (varint) format.
    NetworkNbt,
    NbtEncoding::Network
);

impl From<Nbt> for NetworkNbt {
    fn from(value: Nbt) -> Self {
        Self { name: value.name, root: value.root }
    }
}

impl From<NetworkNbt> for Nbt {
    fn from(value: NetworkNbt) -> Self {
        Self { name: value.name, root: value.root }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> NbtTag {
        NbtTag::Compound(vec![
            ("b".into(), NbtTag::Byte(-1)),
            ("s".into(), NbtTag::Short(300)),
            ("i".into(), NbtTag::Int(-70000)),
            ("l".into(), NbtTag::Long(i64::MIN)),
            ("f".into(), NbtTag::Float(1.5)),
            ("d".into(), NbtTag::Double(-2.25)),
            ("ba".into(), NbtTag::ByteArray(vec![1, 2, 3])),
            ("str".into(), NbtTag::String("hello".into())),
            ("list".into(), NbtTag::List(vec![NbtTag::Int(1), NbtTag::Int(2)])),
            ("empty".into(), NbtTag::List(vec![])),
            ("nested".into(), NbtTag::Compound(vec![("x".into(), NbtTag::IntArray(vec![1, -1]))])),
            ("la".into(), NbtTag::LongArray(vec![5, -5])),
        ])
    }

    #[test]
    fn roundtrip_both_encodings() {
        let network = NetworkNbt::new(sample());
        assert_eq!(NetworkNbt::from_bytes(&network.to_bytes().unwrap()).unwrap(), network);
        let little = Nbt::new(sample());
        assert_eq!(Nbt::from_bytes(&little.to_bytes().unwrap()).unwrap(), little);
        assert_ne!(network.to_bytes().unwrap(), little.to_bytes().unwrap());
    }

    #[test]
    fn known_bytes() {
        let tag = NbtTag::Compound(vec![("a".into(), NbtTag::Int(1))]);
        assert_eq!(NetworkNbt::new(tag.clone()).to_bytes().unwrap(), [10, 0, 3, 1, b'a', 2, 0]);
        assert_eq!(Nbt::new(tag).to_bytes().unwrap(), [10, 0, 0, 3, 1, 0, b'a', 1, 0, 0, 0, 0]);
        assert_eq!(NetworkNbt::new(NbtTag::End).to_bytes().unwrap(), [0]);
        assert_eq!(NetworkNbt::from_bytes(&[0]).unwrap().root, NbtTag::End);
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(NetworkNbt::from_bytes(&[13, 0]), Err(ProtocolError::InvalidNbtTagType(13)));
        let mixed = NbtTag::List(vec![NbtTag::Int(1), NbtTag::Byte(1)]);
        assert!(NetworkNbt::new(mixed).to_bytes().is_err());
        // `End` is only meaningful as a root: anywhere else it corrupts the stream.
        assert!(NetworkNbt::new(NbtTag::Compound(vec![("x".into(), NbtTag::End)])).to_bytes().is_err());
        assert!(NetworkNbt::new(NbtTag::List(vec![NbtTag::End])).to_bytes().is_err());
        // Encoding refuses what decoding would refuse.
        let mut deep = NbtTag::Byte(0);
        for _ in 0..MAX_DEPTH + 1 {
            deep = NbtTag::List(vec![deep]);
        }
        assert_eq!(NetworkNbt::new(deep).to_bytes(), Err(ProtocolError::NbtTooDeep));
        // Nested lists announcing huge counts must not reserve memory for them.
        let mut bytes = vec![9u8, 0];
        for _ in 0..400 {
            bytes.extend_from_slice(&[9, 0xfe, 0xff, 0xff, 0xff, 0x0f]);
        }
        bytes.resize(1 << 16, 0);
        assert!(NetworkNbt::from_bytes(&bytes).is_err());
        // 100 000 nested lists of lists: rejected without touching the stack.
        let mut bytes = vec![9u8, 0];
        for _ in 0..100_000 {
            bytes.extend_from_slice(&[9, 2]);
        }
        assert_eq!(NetworkNbt::from_bytes(&bytes), Err(ProtocolError::NbtTooDeep));
        // A list announcing elements of an unknown type.
        assert_eq!(NetworkNbt::from_bytes(&[9, 0, 99, 2]), Err(ProtocolError::InvalidNbtTagType(99)));
        // Truncated compound.
        assert!(matches!(NetworkNbt::from_bytes(&[10, 0, 3, 1, b'a']), Err(ProtocolError::UnexpectedEof { .. })));
    }

    #[test]
    fn deep_but_legal_nesting_round_trips() {
        let mut tag = NbtTag::Compound(vec![("leaf".into(), NbtTag::Byte(1))]);
        for depth in 0..MAX_DEPTH - 1 {
            tag = if depth % 2 == 0 { NbtTag::List(vec![tag]) } else { NbtTag::Compound(vec![("c".into(), tag)]) };
        }
        let nbt = NetworkNbt::new(tag);
        let bytes = nbt.to_bytes().unwrap();
        assert_eq!(NetworkNbt::from_bytes(&bytes).unwrap(), nbt);
    }
}
