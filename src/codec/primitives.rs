//! [`BedrockCodec`] implementations for the protocol primitives.

use super::error::{ProtocolError, Result};
use super::io::*;
use super::BedrockCodec;
use bytes::{Buf, BufMut};
use std::fmt;

macro_rules! scalar_codec {
    ($($ty:ty => $put:ident, $get:ident;)*) => {$(
        impl BedrockCodec for $ty {
            #[inline]
            fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()> {
                $put(buf, *self);
                Ok(())
            }
            #[inline]
            fn deserialize<B: Buf>(buf: &mut B) -> Result<Self> {
                $get(buf)
            }
        }
    )*};
}

// Plain Rust scalars use the fixed-width little-endian encoding.
scalar_codec! {
    u8 => put_u8, get_u8;
    i8 => put_i8, get_i8;
    u16 => put_u16_le, get_u16_le;
    i16 => put_i16_le, get_i16_le;
    u32 => put_u32_le, get_u32_le;
    i32 => put_i32_le, get_i32_le;
    u64 => put_u64_le, get_u64_le;
    i64 => put_i64_le, get_i64_le;
    f32 => put_f32_le, get_f32_le;
    f64 => put_f64_le, get_f64_le;
    bool => put_bool, get_bool;
}

macro_rules! var_wrapper {
    ($($(#[$doc:meta])* $name:ident($ty:ty) => $put:ident, $get:ident;)*) => {$(
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        pub struct $name(pub $ty);

        impl BedrockCodec for $name {
            #[inline]
            fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()> {
                $put(buf, self.0);
                Ok(())
            }
            #[inline]
            fn deserialize<B: Buf>(buf: &mut B) -> Result<Self> {
                $get(buf).map(Self)
            }
        }

        impl From<$ty> for $name {
            #[inline]
            fn from(value: $ty) -> Self {
                Self(value)
            }
        }

        impl From<$name> for $ty {
            #[inline]
            fn from(value: $name) -> Self {
                value.0
            }
        }
    )*};
}

var_wrapper! {
    /// Unsigned 32-bit integer encoded as LEB128 (1 to 5 bytes).
    VarUInt32(u32) => put_u32_var, get_u32_var;
    /// Signed 32-bit integer encoded as ZigZag + LEB128 (1 to 5 bytes).
    VarInt32(i32) => put_i32_var, get_i32_var;
    /// Unsigned 64-bit integer encoded as LEB128 (1 to 10 bytes).
    VarUInt64(u64) => put_u64_var, get_u64_var;
    /// Signed 64-bit integer encoded as ZigZag + LEB128 (1 to 10 bytes).
    VarInt64(i64) => put_i64_var, get_i64_var;
}

/// `String`: UTF-8 bytes prefixed by their length as a `VarUInt32`.
impl BedrockCodec for String {
    fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()> {
        put_string(buf, self)
    }
    fn deserialize<B: Buf>(buf: &mut B) -> Result<Self> {
        get_string(buf)
    }
}

/// `std::optional<T>`: a boolean presence flag followed by the value.
impl<T: BedrockCodec> BedrockCodec for Option<T> {
    fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()> {
        match self {
            Some(value) => {
                put_bool(buf, true);
                value.serialize(buf)
            }
            None => {
                put_bool(buf, false);
                Ok(())
            }
        }
    }
    fn deserialize<B: Buf>(buf: &mut B) -> Result<Self> {
        if get_bool(buf)? {
            Ok(Some(T::deserialize(buf)?))
        } else {
            Ok(None)
        }
    }
}

/// `std::vector<T>`: a `VarUInt32` element count followed by the elements.
impl<T: BedrockCodec> BedrockCodec for Vec<T> {
    fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()> {
        put_len_var(buf, self.len())?;
        self.iter().try_for_each(|item| item.serialize(buf))
    }
    fn deserialize<B: Buf>(buf: &mut B) -> Result<Self> {
        let len = get_len_var(buf)?;
        get_vec(buf, len, T::deserialize)
    }
}

/// `std::array<T, N>`: the elements without any prefix.
impl<T: BedrockCodec, const N: usize> BedrockCodec for [T; N] {
    fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()> {
        self.iter().try_for_each(|item| item.serialize(buf))
    }
    fn deserialize<B: Buf>(buf: &mut B) -> Result<Self> {
        get_array(buf, T::deserialize)
    }
}

impl<T: BedrockCodec> BedrockCodec for Box<T> {
    fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()> {
        (**self).serialize(buf)
    }
    fn deserialize<B: Buf>(buf: &mut B) -> Result<Self> {
        T::deserialize(buf).map(Box::new)
    }
}

/// 128-bit UUID, stored in canonical (big-endian, RFC 4122) byte order.
///
/// On the wire Bedrock writes the most significant 64 bits then the least
/// significant 64 bits, each as a little-endian `u64`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Uuid(pub [u8; 16]);

impl Uuid {
    pub const NIL: Self = Self([0; 16]);

    #[inline]
    pub const fn from_u128(value: u128) -> Self {
        Self(value.to_be_bytes())
    }

    #[inline]
    pub const fn as_u128(&self) -> u128 {
        u128::from_be_bytes(self.0)
    }

    #[inline]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl From<u128> for Uuid {
    fn from(value: u128) -> Self {
        Self::from_u128(value)
    }
}

impl From<[u8; 16]> for Uuid {
    fn from(value: [u8; 16]) -> Self {
        Self(value)
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = self.as_u128();
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            (v >> 96) as u32,
            (v >> 80) as u16,
            (v >> 64) as u16,
            (v >> 48) as u16,
            v & 0xffff_ffff_ffff
        )
    }
}

impl fmt::Debug for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Uuid({self})")
    }
}

impl BedrockCodec for Uuid {
    fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()> {
        let v = self.as_u128();
        put_u64_le(buf, (v >> 64) as u64);
        put_u64_le(buf, v as u64);
        Ok(())
    }
    fn deserialize<B: Buf>(buf: &mut B) -> Result<Self> {
        let most = get_u64_le(buf)? as u128;
        let least = get_u64_le(buf)? as u128;
        Ok(Self::from_u128((most << 64) | least))
    }
}

/// Two little-endian `f32`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

impl BedrockCodec for Vec2 {
    fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()> {
        put_f32_le(buf, self.x);
        put_f32_le(buf, self.y);
        Ok(())
    }
    fn deserialize<B: Buf>(buf: &mut B) -> Result<Self> {
        Ok(Self { x: get_f32_le(buf)?, y: get_f32_le(buf)? })
    }
}

/// Three little-endian `f32`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
}

impl BedrockCodec for Vec3 {
    fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()> {
        put_f32_le(buf, self.x);
        put_f32_le(buf, self.y);
        put_f32_le(buf, self.z);
        Ok(())
    }
    fn deserialize<B: Buf>(buf: &mut B) -> Result<Self> {
        Ok(Self { x: get_f32_le(buf)?, y: get_f32_le(buf)?, z: get_f32_le(buf)? })
    }
}

/// Block position: `x` and `z` as `VarInt32`, `y` as `VarUInt32`
/// (the classic "network block position").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}

impl BedrockCodec for BlockPos {
    fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()> {
        put_i32_var(buf, self.x);
        put_u32_var(buf, self.y as u32);
        put_i32_var(buf, self.z);
        Ok(())
    }
    fn deserialize<B: Buf>(buf: &mut B) -> Result<Self> {
        Ok(Self { x: get_i32_var(buf)?, y: get_u32_var(buf)? as i32, z: get_i32_var(buf)? })
    }
}

/// Sub-chunk position: three `VarInt32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SubChunkPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl SubChunkPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}

impl BedrockCodec for SubChunkPos {
    fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()> {
        put_i32_var(buf, self.x);
        put_i32_var(buf, self.y);
        put_i32_var(buf, self.z);
        Ok(())
    }
    fn deserialize<B: Buf>(buf: &mut B) -> Result<Self> {
        Ok(Self { x: get_i32_var(buf)?, y: get_i32_var(buf)?, z: get_i32_var(buf)? })
    }
}

/// Header that precedes every game packet inside a batch: a `VarUInt32`
/// packing the packet id (10 bits) and the two split-screen sub-client ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PacketHeader {
    pub packet_id: u32,
    pub sender_sub_client: u8,
    pub target_sub_client: u8,
}

impl PacketHeader {
    pub const fn new(packet_id: u32) -> Self {
        Self { packet_id, sender_sub_client: 0, target_sub_client: 0 }
    }
}

impl BedrockCodec for PacketHeader {
    fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()> {
        if self.packet_id > 0x3ff {
            return Err(ProtocolError::IntegerOutOfRange { type_name: "packet id", value: self.packet_id as i128 });
        }
        let raw = self.packet_id
            | ((self.sender_sub_client as u32 & 0x3) << 10)
            | ((self.target_sub_client as u32 & 0x3) << 12);
        put_u32_var(buf, raw);
        Ok(())
    }
    fn deserialize<B: Buf>(buf: &mut B) -> Result<Self> {
        let raw = get_u32_var(buf)?;
        Ok(Self {
            packet_id: raw & 0x3ff,
            sender_sub_client: ((raw >> 10) & 0x3) as u8,
            target_sub_client: ((raw >> 12) & 0x3) as u8,
        })
    }
}

/// Implemented by every generated protocol enum.
pub trait ProtocolEnum: Copy + Sized + 'static {
    /// Name of the enum, for diagnostics.
    const NAME: &'static str;

    /// Numeric value of the variant, widened to `i64`.
    fn to_raw(self) -> i64;

    /// Variant holding this numeric value, if any.
    fn from_raw(raw: i64) -> Option<Self>;

    /// Name of the variant as documented by Mojang.
    fn name(self) -> &'static str;

    /// Variant with this documented name (ASCII case-insensitive), if any.
    fn from_name(name: &str) -> Option<Self>;

    /// Name of the variant as written on the wire when the enum is
    /// serialized as a string: the documented name in lower case.
    fn wire_name(self) -> String {
        self.name().to_ascii_lowercase()
    }

    /// Like [`ProtocolEnum::from_raw`] but with a protocol error.
    fn from_wire(raw: i64) -> Result<Self> {
        Self::from_raw(raw).ok_or(ProtocolError::InvalidEnumValue { enum_name: Self::NAME, value: raw })
    }

    /// Like [`ProtocolEnum::from_name`] but with a protocol error.
    fn from_wire_name(name: &str) -> Result<Self> {
        Self::from_name(name)
            .ok_or_else(|| ProtocolError::InvalidEnumName { enum_name: Self::NAME, name: name.to_owned() })
    }
}

/// An enum field for which Mojang allows values unknown to this protocol
/// version (`"Allow unknown enum values"`): unknown values are preserved
/// instead of failing the whole packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaybeUnknown<E> {
    Known(E),
    Unknown(i64),
}

impl<E: ProtocolEnum> MaybeUnknown<E> {
    #[inline]
    pub fn from_raw(raw: i64) -> Self {
        match E::from_raw(raw) {
            Some(known) => Self::Known(known),
            None => Self::Unknown(raw),
        }
    }

    #[inline]
    pub fn to_raw(self) -> i64 {
        match self {
            Self::Known(known) => known.to_raw(),
            Self::Unknown(raw) => raw,
        }
    }

    #[inline]
    pub fn known(self) -> Option<E> {
        match self {
            Self::Known(known) => Some(known),
            Self::Unknown(_) => None,
        }
    }
}

impl<E: Default> Default for MaybeUnknown<E> {
    fn default() -> Self {
        Self::Known(E::default())
    }
}

impl<E> From<E> for MaybeUnknown<E> {
    fn from(value: E) -> Self {
        Self::Known(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T: BedrockCodec + PartialEq + fmt::Debug>(value: T, expected: &[u8]) {
        let bytes = value.to_bytes().unwrap();
        assert_eq!(bytes, expected, "encoding of {value:?}");
        assert_eq!(T::from_bytes(&bytes).unwrap(), value);
    }

    #[test]
    fn fixed_scalars_are_little_endian() {
        roundtrip(0x12u8, &[0x12]);
        roundtrip(-2i8, &[0xfe]);
        roundtrip(0x1234u16, &[0x34, 0x12]);
        roundtrip(-2i16, &[0xfe, 0xff]);
        roundtrip(0x1234_5678u32, &[0x78, 0x56, 0x34, 0x12]);
        roundtrip(-2i32, &[0xfe, 0xff, 0xff, 0xff]);
        roundtrip(0x0102_0304_0506_0708u64, &[8, 7, 6, 5, 4, 3, 2, 1]);
        roundtrip(-2i64, &[0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        roundtrip(1.0f32, &[0, 0, 0x80, 0x3f]);
        roundtrip(1.0f64, &[0, 0, 0, 0, 0, 0, 0xf0, 0x3f]);
        roundtrip(true, &[1]);
        roundtrip(false, &[0]);
    }

    #[test]
    fn big_endian_helpers() {
        let mut buf = Vec::new();
        put_i32_be(&mut buf, 0x0102_0304);
        assert_eq!(buf, [1, 2, 3, 4]);
        assert_eq!(get_i32_be(&mut buf.as_slice()).unwrap(), 0x0102_0304);
    }

    #[test]
    fn varints() {
        roundtrip(VarUInt32(0), &[0]);
        roundtrip(VarUInt32(127), &[0x7f]);
        roundtrip(VarUInt32(128), &[0x80, 0x01]);
        roundtrip(VarUInt32(300), &[0xac, 0x02]);
        roundtrip(VarUInt32(u32::MAX), &[0xff, 0xff, 0xff, 0xff, 0x0f]);
        roundtrip(VarInt32(0), &[0]);
        roundtrip(VarInt32(-1), &[1]);
        roundtrip(VarInt32(1), &[2]);
        roundtrip(VarInt32(-2), &[3]);
        roundtrip(VarInt32(i32::MAX), &[0xfe, 0xff, 0xff, 0xff, 0x0f]);
        roundtrip(VarInt32(i32::MIN), &[0xff, 0xff, 0xff, 0xff, 0x0f]);
        roundtrip(VarUInt64(u64::MAX), &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]);
        roundtrip(VarInt64(-1), &[1]);
        roundtrip(VarInt64(i64::MIN), &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]);
        roundtrip(VarInt64(i64::MAX), &[0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]);
    }

    #[test]
    fn varint_errors() {
        assert_eq!(
            VarUInt32::from_bytes(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
            Err(ProtocolError::VarIntTooLong { max_bytes: 5 })
        );
        assert_eq!(VarUInt32::from_bytes(&[0x80]), Err(ProtocolError::UnexpectedEof { needed: 1, remaining: 0 }));
        assert!(matches!(get_u8_var(&mut [0x80u8, 0x02].as_slice()), Err(ProtocolError::IntegerOutOfRange { .. })));
        assert_eq!(get_i8_var(&mut [0x01u8].as_slice()), Ok(-1));
    }

    #[test]
    fn strings_and_containers() {
        roundtrip(String::from("héllo"), &[6, b'h', 0xc3, 0xa9, b'l', b'l', b'o']);
        roundtrip(vec![1u16, 2], &[2, 1, 0, 2, 0]);
        roundtrip(Some(7u8), &[1, 7]);
        roundtrip(None::<u8>, &[0]);
        roundtrip([1u8, 2, 3], &[1, 2, 3]);
        roundtrip(Box::new(5u8), &[5]);
        assert_eq!(String::from_bytes(&[2, 0xff, 0xff]), Err(ProtocolError::InvalidUtf8));
        assert_eq!(String::from_bytes(&[5, b'a']), Err(ProtocolError::UnexpectedEof { needed: 5, remaining: 1 }));
        // A hostile length must fail cleanly instead of allocating.
        assert!(Vec::<u64>::from_bytes(&[0xff, 0xff, 0xff, 0xff, 0x0f]).is_err());
        assert_eq!(u8::from_bytes(&[1, 2]), Err(ProtocolError::TrailingBytes { remaining: 1 }));
        // Elements that consume no input cannot be used to spin the decoder.
        let spin = get_vec(&mut (&[] as &[u8]), usize::MAX, |_| Ok(()));
        assert_eq!(spin, Err(ProtocolError::LengthOutOfRange { length: usize::MAX as u64 }));
        // Large elements do not multiply the pre-allocation.
        let big = get_vec(&mut [0u8; 64].as_slice(), 1 << 30, |buf| Ok([get_u8(buf)?; 4096]));
        assert!(matches!(big, Err(ProtocolError::UnexpectedEof { .. })));
    }

    #[test]
    fn uuid_layout() {
        let uuid = Uuid::from_u128(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff);
        assert_eq!(uuid.to_string(), "00112233-4455-6677-8899-aabbccddeeff");
        roundtrip(
            uuid,
            &[0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x00, 0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88],
        );
    }

    #[test]
    fn positions() {
        roundtrip(Vec2::new(1.0, -1.0), &[0, 0, 0x80, 0x3f, 0, 0, 0x80, 0xbf]);
        roundtrip(Vec3::new(0.0, 1.0, 0.0), &[0, 0, 0, 0, 0, 0, 0x80, 0x3f, 0, 0, 0, 0]);
        roundtrip(BlockPos::new(-1, 64, 1), &[1, 64, 2]);
        roundtrip(BlockPos::new(0, -1, 0), &[0, 0xff, 0xff, 0xff, 0xff, 0x0f, 0]);
        roundtrip(SubChunkPos::new(-1, -2, 3), &[1, 3, 6]);
    }

    #[test]
    fn packet_header() {
        roundtrip(PacketHeader::new(0x8f), &[0x8f, 0x01]);
        roundtrip(PacketHeader { packet_id: 1, sender_sub_client: 1, target_sub_client: 2 }, &[0x81, 0x48]);
        assert!(PacketHeader::new(0x400).to_bytes().is_err());
    }
}
