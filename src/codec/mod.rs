//! Binary codec of the Bedrock protocol: the [`BedrockCodec`] trait, the
//! [`ProtocolError`] type and all the wire primitives.

pub mod connection;
pub mod error;
pub mod framing;
pub mod io;
pub mod nbt;
pub mod primitives;
pub mod string;

pub use bytes::{Buf, BufMut};
pub use error::{ProtocolError, Result};
pub use io::*;
pub use connection::ConnectionRequest;
pub use framing::{
    decode_batch, decode_framed, encode_batch, encode_framed, encode_framed_with, frames, get_batch_envelope,
    put_batch_envelope, split_frame, Compression, Frames, RawFrame, GAME_PACKET_ID,
};
pub use nbt::{Nbt, NbtTag, NetworkNbt};
pub use primitives::{
    BlockPos, ChunkPos, MaybeUnknown, NetworkBlockPos, PacketHeader, ProtocolEnum, SubChunkPos, Uuid,
    VarInt32, VarInt64, VarUInt32, VarUInt64, Vec2, Vec3,
};
pub use string::BedrockString;

/// Checks a value against the limits Mojang documents for it (`maxLength`,
/// `minItems` / `maxItems`, `minimum` / `maximum`, `maxProperties`).
///
/// Encoding and decoding never apply these limits: they describe what the
/// game accepts, not how bytes are laid out. Call [`Validate::validate`] on
/// what you receive from a peer, or on what you are about to send.
pub trait Validate {
    /// Returns the first violated constraint, as `ProtocolError::ConstraintViolated`.
    fn validate(&self) -> Result<()>;
}

/// A value that can be written to and read from the Bedrock wire format.
pub trait BedrockCodec: Sized {
    /// Appends the wire representation of `self` to `buf`.
    fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()>;

    /// Reads a value from the front of `buf`, advancing it.
    fn deserialize<B: Buf>(buf: &mut B) -> Result<Self>;

    /// Serializes into a fresh `Vec<u8>`.
    fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.serialize(&mut out)?;
        Ok(out)
    }

    /// Deserializes from `bytes`, failing if any byte is left over.
    fn from_bytes(mut bytes: &[u8]) -> Result<Self> {
        let value = Self::deserialize(&mut bytes)?;
        if bytes.is_empty() {
            Ok(value)
        } else {
            Err(ProtocolError::TrailingBytes { remaining: bytes.len() })
        }
    }
}
