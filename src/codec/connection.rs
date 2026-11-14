//! The connection request carried by `LoginPacket` and `SubClientLoginPacket`.

use super::error::{ProtocolError, Result};
use super::io::{ensure, get_bytes, get_len_var, get_u32_le, put_len_var, put_u32_le};
use super::string::BedrockString;
use super::BedrockCodec;
use bytes::{Buf, BufMut};

/// Login payload.
///
/// Mojang's schema types the field as a plain `string`; that string is in fact
/// an envelope holding two length-prefixed documents:
///
/// ```text
/// VarUInt32  total length of what follows
///   u32 LE   length of the authentication document
///   bytes    authentication document (JSON)
///   u32 LE   length of the client data token
///   bytes    client data token (JWT: skin, device, language...)
/// ```
///
/// Both documents are kept as raw bytes, so decoding and re-encoding are
/// byte-exact whatever they contain. The helpers below interpret them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConnectionRequest {
    /// JSON describing how the player authenticated. Either
    /// `{"chain": ["<jwt>", ...]}` or, since the authentication rework,
    /// `{"AuthenticationType": n, "Certificate": "<that same JSON, as a string>", "Token": "..."}`.
    pub auth_info: BedrockString,
    /// The client data JWT, signed with the key certified by the chain.
    pub client_data: BedrockString,
}

fn invalid(reason: &'static str) -> ProtocolError {
    ProtocolError::InvalidConnectionRequest(reason)
}

impl ConnectionRequest {
    pub fn new(auth_info: impl Into<BedrockString>, client_data: impl Into<BedrockString>) -> Self {
        Self { auth_info: auth_info.into(), client_data: client_data.into() }
    }

    /// Builds a request in the classic `{"chain": [...]}` form.
    pub fn from_chain<S: AsRef<str>>(chain: &[S], client_data_jwt: &str) -> Self {
        let chain: Vec<&str> = chain.iter().map(AsRef::as_ref).collect();
        let auth_info = serde_json::json!({ "chain": chain }).to_string();
        Self::new(auth_info, client_data_jwt)
    }

    /// The authentication document as text.
    pub fn auth_info_json(&self) -> Result<&str> {
        self.auth_info.as_str().ok_or(ProtocolError::InvalidUtf8)
    }

    /// The client data token as text (three base64url segments separated by dots).
    pub fn client_data_jwt(&self) -> Result<&str> {
        self.client_data.as_str().ok_or(ProtocolError::InvalidUtf8)
    }

    fn auth_document(&self) -> Result<serde_json::Value> {
        serde_json::from_slice(self.auth_info.as_bytes()).map_err(|_| invalid("authentication document is not JSON"))
    }

    /// The certificate chain: the JWTs that link the client's key to Mojang's.
    ///
    /// Handles both layouts: a top-level `chain` array, and a `Certificate`
    /// string that itself contains the `{"chain": [...]}` document.
    pub fn chain(&self) -> Result<Vec<String>> {
        let document = self.auth_document()?;
        let nested;
        let holder = match document.get("chain") {
            Some(_) => &document,
            None => {
                let certificate = document
                    .get("Certificate")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| invalid("neither `chain` nor `Certificate` is present"))?;
                nested = serde_json::from_str::<serde_json::Value>(certificate)
                    .map_err(|_| invalid("`Certificate` is not JSON"))?;
                &nested
            }
        };
        let chain = holder
            .get("chain")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| invalid("`chain` is not an array"))?;
        chain
            .iter()
            .map(|token| token.as_str().map(str::to_owned).ok_or_else(|| invalid("`chain` holds a non-string entry")))
            .collect()
    }

    /// `AuthenticationType` of the reworked layout, when present.
    pub fn authentication_type(&self) -> Result<Option<i64>> {
        Ok(self.auth_document()?.get("AuthenticationType").and_then(serde_json::Value::as_i64))
    }

    /// `Token` of the reworked layout, when present and not empty.
    pub fn token(&self) -> Result<Option<String>> {
        let document = self.auth_document()?;
        let token = document.get("Token").and_then(serde_json::Value::as_str).filter(|token| !token.is_empty());
        Ok(token.map(str::to_owned))
    }

    /// Size of the envelope content, without its `VarUInt32` prefix.
    fn inner_len(&self) -> usize {
        8 + self.auth_info.len() + self.client_data.len()
    }
}

fn put_document<B: BufMut>(buf: &mut B, document: &BedrockString) -> Result<()> {
    let len = u32::try_from(document.len())
        .map_err(|_| ProtocolError::LengthOutOfRange { length: document.len() as u64 })?;
    put_u32_le(buf, len);
    buf.put_slice(document.as_bytes());
    Ok(())
}

fn get_document<B: Buf>(buf: &mut B) -> Result<BedrockString> {
    let len = get_u32_le(buf)? as usize;
    get_bytes(buf, len).map(BedrockString::from)
}

impl BedrockCodec for ConnectionRequest {
    fn serialize<B: BufMut>(&self, buf: &mut B) -> Result<()> {
        put_len_var(buf, self.inner_len())?;
        put_document(buf, &self.auth_info)?;
        put_document(buf, &self.client_data)
    }

    fn deserialize<B: Buf>(buf: &mut B) -> Result<Self> {
        let len = get_len_var(buf)?;
        ensure(buf, len)?;
        // Confine the two documents to the envelope: their own length
        // prefixes can neither read past it nor leave part of it unread.
        let mut envelope = buf.take(len);
        let auth_info = get_document(&mut envelope)?;
        let client_data = get_document(&mut envelope)?;
        if envelope.has_remaining() {
            return Err(ProtocolError::TrailingBytes { remaining: envelope.remaining() });
        }
        Ok(Self { auth_info, client_data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_byte_exact() {
        let request = ConnectionRequest::new("AB", "xyz");
        let wire = request.to_bytes().unwrap();
        assert_eq!(wire, [13, 2, 0, 0, 0, b'A', b'B', 3, 0, 0, 0, b'x', b'y', b'z']);
        assert_eq!(ConnectionRequest::from_bytes(&wire).unwrap(), request);
    }

    #[test]
    fn length_prefixes_are_not_text() {
        // 200 = 0xc8: as a length prefix this byte is not valid UTF-8, which
        // is exactly why the envelope cannot be decoded as a string.
        let request = ConnectionRequest::new("a".repeat(200), "");
        let wire = request.to_bytes().unwrap();
        assert_eq!(&wire[..6], [0xd0, 0x01, 0xc8, 0, 0, 0]);
        assert_eq!(ConnectionRequest::from_bytes(&wire).unwrap(), request);
    }

    #[test]
    fn chain_in_both_layouts() {
        let classic = ConnectionRequest::from_chain(&["a.b.c", "d.e.f"], "h.p.s");
        assert_eq!(classic.auth_info_json().unwrap(), r#"{"chain":["a.b.c","d.e.f"]}"#);
        assert_eq!(classic.chain().unwrap(), ["a.b.c", "d.e.f"]);
        assert_eq!(classic.client_data_jwt().unwrap(), "h.p.s");
        assert_eq!(classic.authentication_type().unwrap(), None);

        let reworked = ConnectionRequest::new(
            r#"{"AuthenticationType":0,"Certificate":"{\"chain\":[\"a.b.c\"]}","Token":"tok"}"#,
            "h.p.s",
        );
        assert_eq!(reworked.chain().unwrap(), ["a.b.c"]);
        assert_eq!(reworked.authentication_type().unwrap(), Some(0));
        assert_eq!(reworked.token().unwrap().as_deref(), Some("tok"));
    }

    #[test]
    fn malformed_envelopes_are_rejected() {
        // Inner length larger than the envelope.
        assert!(matches!(
            ConnectionRequest::from_bytes(&[5, 9, 0, 0, 0, b'A']),
            Err(ProtocolError::UnexpectedEof { .. })
        ));
        // Envelope longer than the buffer.
        assert!(matches!(ConnectionRequest::from_bytes(&[50, 0, 0, 0, 0]), Err(ProtocolError::UnexpectedEof { .. })));
        // Bytes left inside the envelope after the two documents.
        assert_eq!(
            ConnectionRequest::from_bytes(&[9, 0, 0, 0, 0, 0, 0, 0, 0, 7]),
            Err(ProtocolError::TrailingBytes { remaining: 1 })
        );
        // Documents that are not what the helpers expect.
        let request = ConnectionRequest::new("not json", vec![0xff]);
        assert!(matches!(request.chain(), Err(ProtocolError::InvalidConnectionRequest(_))));
        assert_eq!(request.client_data_jwt(), Err(ProtocolError::InvalidUtf8));
        assert!(ConnectionRequest::new(r#"{"chain":[1]}"#, "").chain().is_err());
    }
}
