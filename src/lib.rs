//! # mcbrs-protocol
//!
//! Minecraft: Bedrock Edition network protocol for MCBE-RS.
//!
//! * [`codec`] is hand-written: the [`BedrockCodec`] trait, the
//!   [`ProtocolError`] type and every binary primitive of the protocol
//!   (little-endian scalars, varints, strings, UUIDs, vectors, NBT, ...).
//! * [`generated`] is produced by the `proto-codegen` tool from the JSON schemas
//!   published by Mojang (<https://github.com/Mojang/bedrock-protocol-docs>).
//!   **Never edit it by hand**: run `cargo run --bin proto-codegen` instead.
//!
//! ```
//! use mcbrs_protocol::Packet;
//!
//! let id = Packet::IDS[0];
//! let packet = Packet::default_for_id(id).unwrap();
//! let mut bytes = Vec::new();
//! packet.encode(&mut bytes).unwrap();
//! let decoded = Packet::decode(id, &mut bytes.as_slice()).unwrap();
//! assert_eq!(packet, decoded);
//! ```

pub mod codec;
#[rustfmt::skip]
pub mod generated;
#[cfg(test)]
mod testing;

pub use codec::{BedrockCodec, ProtocolError, Result};
pub use generated::packets::Packet;
pub use generated::{enums, packets, types};
