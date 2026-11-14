//! Packet framing: what sits between the transport and the packet bodies.
//!
//! A *frame* is one packet as it appears inside a batch:
//!
//! ```text
//! VarUInt32  length of the rest of the frame
//!   VarUInt32  packet header (id, sender and target sub-client)
//!   bytes      packet body
//! ```
//!
//! A *batch* is any number of frames back to back. On RakNet a batch is sent
//! as one game packet: the byte [`GAME_PACKET_ID`], then (once compression has
//! been negotiated with `NetworkSettingsPacket`) one [`Compression`] byte, then
//! the possibly compressed batch. Other transports wrap the batch their own
//! way; whatever they do, what they deliver ends up as a batch.
//!
//! This module produces and consumes frames and batches, and reads / writes
//! the two envelope bytes. Compression itself is left to the caller: it is a
//! transport policy (algorithm, threshold, decompression limits) and pulling a
//! codec in here would force it on every user.

use super::error::{ProtocolError, Result};
use super::io::{ensure, get_len_var, get_u8, put_len_var, put_u8};
use super::primitives::PacketHeader;
use super::BedrockCodec;
use crate::generated::packets::Packet;
use bytes::{Buf, BufMut};

/// First byte of a RakNet message that carries a batch.
pub const GAME_PACKET_ID: u8 = 0xfe;

/// Compression applied to a batch, as announced by the byte that follows
/// [`GAME_PACKET_ID`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Compression {
    /// Raw deflate (no zlib header).
    Deflate,
    Snappy,
    /// The batch follows as is.
    None,
}

impl Compression {
    pub const fn to_byte(self) -> u8 {
        match self {
            Self::Deflate => 0x00,
            Self::Snappy => 0x01,
            Self::None => 0xff,
        }
    }

    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0x00 => Some(Self::Deflate),
            0x01 => Some(Self::Snappy),
            0xff => Some(Self::None),
            _ => None,
        }
    }
}

/// Writes the RakNet envelope: [`GAME_PACKET_ID`], then the compression byte
/// if compression has been negotiated (`None` before `NetworkSettingsPacket`).
pub fn put_batch_envelope<B: BufMut>(buf: &mut B, compression: Option<Compression>) {
    put_u8(buf, GAME_PACKET_ID);
    if let Some(compression) = compression {
        put_u8(buf, compression.to_byte());
    }
}

/// Reads the RakNet envelope. `compression_negotiated` says whether a
/// compression byte is expected after [`GAME_PACKET_ID`].
pub fn get_batch_envelope<B: Buf>(buf: &mut B, compression_negotiated: bool) -> Result<Option<Compression>> {
    let id = get_u8(buf)?;
    if id != GAME_PACKET_ID {
        return Err(ProtocolError::InvalidBatch("message does not start with the game packet id"));
    }
    if !compression_negotiated {
        return Ok(None);
    }
    let byte = get_u8(buf)?;
    Compression::from_byte(byte).map(Some).ok_or(ProtocolError::InvalidBatch("unknown compression algorithm"))
}

/// Writes one frame with an explicit header (sub-client ids for split screen).
pub fn encode_framed_with<B: BufMut>(header: PacketHeader, packet: &Packet, buf: &mut B) -> Result<()> {
    if header.packet_id != packet.packet_id() {
        return Err(ProtocolError::InvalidBatch("header and packet disagree on the packet id"));
    }
    // The length precedes the content, so the content is built first.
    let mut frame = Vec::new();
    header.serialize(&mut frame)?;
    packet.encode(&mut frame)?;
    put_len_var(buf, frame.len())?;
    buf.put_slice(&frame);
    Ok(())
}

/// Writes one frame: length, header, body.
pub fn encode_framed<B: BufMut>(packet: &Packet, buf: &mut B) -> Result<()> {
    encode_framed_with(PacketHeader::new(packet.packet_id()), packet, buf)
}

/// One frame of a batch, not decoded yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawFrame<'a> {
    pub header: PacketHeader,
    /// The packet body, exactly as delimited by the frame length.
    pub body: &'a [u8],
}

impl RawFrame<'_> {
    /// Decodes the body. The body must be consumed entirely: bytes left over
    /// mean the packet was not understood, and are reported as an error
    /// rather than silently dropped.
    pub fn decode(&self) -> Result<Packet> {
        let mut body = self.body;
        let packet = Packet::decode(self.header.packet_id, &mut body)?;
        if body.is_empty() {
            Ok(packet)
        } else {
            Err(ProtocolError::TrailingBytes { remaining: body.len() }.in_packet(packet.name()))
        }
    }
}

/// Splits one frame off the front of `input`, without decoding its body.
///
/// Because the frame carries its own length, a packet this crate cannot
/// decode (unknown id, newer layout) never desynchronizes the rest of the
/// batch: the caller can skip it and go on.
pub fn split_frame<'a>(input: &mut &'a [u8]) -> Result<RawFrame<'a>> {
    let len = get_len_var(input)?;
    ensure(input, len)?;
    let (mut frame, rest) = input.split_at(len);
    *input = rest;
    let header = PacketHeader::deserialize(&mut frame)?;
    Ok(RawFrame { header, body: frame })
}

/// Reads one frame and decodes its packet.
pub fn decode_framed(input: &mut &[u8]) -> Result<(PacketHeader, Packet)> {
    let frame = split_frame(input)?;
    Ok((frame.header, frame.decode()?))
}

/// Iterator over the frames of a batch; see [`frames`].
#[derive(Debug, Clone)]
pub struct Frames<'a> {
    rest: &'a [u8],
    failed: bool,
}

impl<'a> Iterator for Frames<'a> {
    type Item = Result<RawFrame<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.rest.is_empty() {
            return None;
        }
        let frame = split_frame(&mut self.rest);
        // A broken length prefix leaves no way to find the next frame.
        self.failed = frame.is_err();
        Some(frame)
    }
}

/// Iterates over the frames of a decompressed batch without decoding them,
/// for callers that want to route, filter or skip packets.
pub fn frames(batch: &[u8]) -> Frames<'_> {
    Frames { rest: batch, failed: false }
}

/// Writes a batch: every packet as a frame, back to back.
pub fn encode_batch<'p, B: BufMut>(packets: impl IntoIterator<Item = &'p Packet>, buf: &mut B) -> Result<()> {
    packets.into_iter().try_for_each(|packet| encode_framed(packet, buf))
}

/// Decodes every packet of a decompressed batch, failing on the first frame
/// that cannot be decoded. Use [`frames`] to handle frames one by one instead.
pub fn decode_batch(batch: &[u8]) -> Result<Vec<(PacketHeader, Packet)>> {
    frames(batch).map(|frame| frame.and_then(|frame| Ok((frame.header, frame.decode()?)))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packets() -> Vec<Packet> {
        Packet::IDS.iter().take(5).map(|&id| Packet::default_for_id(id).unwrap()).collect()
    }

    #[test]
    fn batch_round_trip() {
        let packets = packets();
        let mut batch = Vec::new();
        encode_batch(&packets, &mut batch).unwrap();
        let decoded = decode_batch(&batch).unwrap();
        assert_eq!(decoded.len(), packets.len());
        for ((header, packet), original) in decoded.iter().zip(&packets) {
            assert_eq!(header.packet_id, original.packet_id());
            assert_eq!(packet, original);
        }
        assert_eq!(decode_batch(&[]).unwrap(), []);
    }

    #[test]
    fn frames_are_self_delimiting() {
        let packet = &packets()[0];
        let mut batch = Vec::new();
        encode_framed(packet, &mut batch).unwrap();
        // An unknown packet (id 1023) with a 2-byte body, then the same packet again.
        batch.extend_from_slice(&[4, 0xff, 0x07, 0xaa, 0xbb]);
        encode_framed(packet, &mut batch).unwrap();

        let all: Vec<_> = frames(&batch).collect::<Result<_>>().unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].decode().unwrap(), *packet);
        assert_eq!(all[1].header.packet_id, 1023);
        assert_eq!(all[1].body, [0xaa, 0xbb]);
        assert_eq!(all[1].decode(), Err(ProtocolError::UnknownPacketId(1023)));
        // The unknown frame did not disturb the one after it.
        assert_eq!(all[2].decode().unwrap(), *packet);
        assert!(decode_batch(&batch).is_err());
    }

    #[test]
    fn malformed_frames() {
        // Length larger than what is left.
        assert_eq!(
            split_frame(&mut [9u8, 1].as_slice()),
            Err(ProtocolError::UnexpectedEof { needed: 9, remaining: 1 })
        );
        // Empty frame: no room for a header.
        assert!(split_frame(&mut [0u8].as_slice()).is_err());
        // After a broken frame the iterator stops instead of guessing.
        let mut iter = frames(&[9, 1, 2]);
        assert!(iter.next().unwrap().is_err());
        assert!(iter.next().is_none());
        // Body longer than the packet: reported, not ignored.
        let packet = &packets()[0];
        let mut frame = Vec::new();
        PacketHeader::new(packet.packet_id()).serialize(&mut frame).unwrap();
        packet.encode(&mut frame).unwrap();
        frame.push(0);
        let mut batch = vec![frame.len() as u8];
        batch.extend_from_slice(&frame);
        assert!(matches!(
            decode_framed(&mut batch.as_slice()),
            Err(ProtocolError::Packet { source, .. }) if *source == ProtocolError::TrailingBytes { remaining: 1 }
        ));
    }

    #[test]
    fn header_must_match_packet() {
        let packet = &packets()[0];
        let wrong = PacketHeader::new(packet.packet_id() + 1);
        assert!(encode_framed_with(wrong, packet, &mut Vec::new()).is_err());
    }

    #[test]
    fn envelope() {
        let mut buf = Vec::new();
        put_batch_envelope(&mut buf, None);
        put_batch_envelope(&mut buf, Some(Compression::None));
        put_batch_envelope(&mut buf, Some(Compression::Deflate));
        put_batch_envelope(&mut buf, Some(Compression::Snappy));
        assert_eq!(buf, [0xfe, 0xfe, 0xff, 0xfe, 0x00, 0xfe, 0x01]);
        let mut input = buf.as_slice();
        assert_eq!(get_batch_envelope(&mut input, false), Ok(None));
        assert_eq!(get_batch_envelope(&mut input, true), Ok(Some(Compression::None)));
        assert_eq!(get_batch_envelope(&mut input, true), Ok(Some(Compression::Deflate)));
        assert_eq!(get_batch_envelope(&mut input, true), Ok(Some(Compression::Snappy)));
        assert!(get_batch_envelope(&mut [0x01u8].as_slice(), false).is_err());
        assert!(get_batch_envelope(&mut [0xfeu8, 0x07].as_slice(), true).is_err());
    }
}
