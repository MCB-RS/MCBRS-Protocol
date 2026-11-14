//! The protocol's string type.

use super::error::Result;
use super::io::{get_bytes, get_len_var, put_len_var};
use super::BedrockCodec;
use bytes::{Buf, BufMut};
use std::borrow::Cow;
use std::fmt;

/// A C++ `std::string` as it travels on the wire: a `VarUInt32` byte count
/// followed by that many bytes.
///
/// Mojang's schemas use `string` both for text and for binary payloads (chunk
/// data, NBT blobs, images, signed tokens), and nothing in the schema tells
/// them apart. The bytes are therefore kept as they are: decoding never fails
/// on content that is not UTF-8, and re-encoding is always byte-exact. Use
/// [`BedrockString::as_str`] when the field is text.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct BedrockString(Vec<u8>);

impl BedrockString {
    /// An empty string.
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// The raw bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consumes the string, returning its bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    /// The content as text, or `None` when it is not valid UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    /// The content as text, with invalid sequences replaced by `U+FFFD`.
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }

    /// Length in bytes (what the protocol's length limits count).
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<&str> for BedrockString {
    fn from(value: &str) -> Self {
        Self(value.as_bytes().to_vec())
    }
}

impl From<String> for BedrockString {
    fn from(value: String) -> Self {
        Self(value.into_bytes())
    }
}

impl From<Vec<u8>> for BedrockString {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

impl From<&[u8]> for BedrockString {
    fn from(value: &[u8]) -> Self {
        Self(value.to_vec())
    }
}

impl From<BedrockString> for Vec<u8> {
    fn from(value: BedrockString) -> Self {
        value.0
    }
}

impl AsRef<[u8]> for BedrockString {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl PartialEq<str> for BedrockString {
    fn eq(&self, other: &str) -> bool {
        self.0 == other.as_bytes()
    }
}

impl PartialEq<&str> for BedrockString {
    fn eq(&self, other: &&str) -> bool {
        self.0 == other.as_bytes()
    }
}

impl PartialEq<[u8]> for BedrockString {
    fn eq(&self, other: &[u8]) -> bool {
        self.0 == other
    }
}

/// Text is shown as text; anything else as a byte count, so that logging a
/// packet holding a megabyte of chunk data stays readable.
impl fmt::Debug for BedrockString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.as_str() {
            Some(text) => fmt::Debug::fmt(text, f),
            None => write!(f, "<{} bytes>", self.0.len()),
        }
    }
}

impl fmt::Display for BedrockString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_lossy())
    }
}

impl BedrockCodec for BedrockString {
    fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()> {
        put_len_var(buf, self.0.len())?;
        buf.put_slice(&self.0);
        Ok(())
    }

    fn deserialize<B: Buf>(buf: &mut B) -> Result<Self> {
        let len = get_len_var(buf)?;
        get_bytes(buf, len).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::ProtocolError;

    #[test]
    fn text_and_binary_round_trip() {
        let text = BedrockString::from("héllo");
        assert_eq!(text.to_bytes().unwrap(), [6, b'h', 0xc3, 0xa9, b'l', b'l', b'o']);
        assert_eq!(text.as_str(), Some("héllo"));
        assert_eq!(text, "héllo");
        assert_eq!(format!("{text:?} {text}"), "\"héllo\" héllo");

        // Bytes that are not UTF-8 are data, not an error.
        let blob = BedrockString::from(vec![0xc8, 0x00, 0xff]);
        let wire = blob.to_bytes().unwrap();
        assert_eq!(wire, [3, 0xc8, 0x00, 0xff]);
        let decoded = BedrockString::from_bytes(&wire).unwrap();
        assert_eq!(decoded, blob);
        assert_eq!(decoded.as_str(), None);
        assert_eq!(format!("{decoded:?}"), "<3 bytes>");
    }

    #[test]
    fn truncated_input_is_rejected() {
        assert_eq!(
            BedrockString::from_bytes(&[5, b'a']),
            Err(ProtocolError::UnexpectedEof { needed: 5, remaining: 1 })
        );
    }
}
