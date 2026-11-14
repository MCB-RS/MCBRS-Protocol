//! Byte-exact checks of the handshake packets and of the framing.
//!
//! The expected bytes are written by hand from the layout Mojang documents,
//! cross-checked field by field against gophertunnel's implementation of the
//! same packets (protocol 2225). They are not captures of a live session.
//!
//! These tests name specific packets and fields on purpose: if a protocol
//! update changes the handshake, they must fail and be reviewed.

use mcbrs_protocol::codec::{
    decode_batch, decode_framed, encode_batch, encode_framed, frames, BedrockCodec, BedrockString, ConnectionRequest,
    PacketHeader, ProtocolError, Uuid, Validate,
};
use mcbrs_protocol::enums::{DisconnectFailReason, PacketCompressionAlgorithm, PlayStatus, SctpRecoveryProfile};
use mcbrs_protocol::generated::PROTOCOL_VERSION;
use mcbrs_protocol::packets::*;
use mcbrs_protocol::types::*;
use mcbrs_protocol::Packet;

/// Encodes `packet`, compares with `expected`, and decodes `expected` back.
fn check(packet: Packet, expected: &[u8]) {
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    assert_eq!(bytes, expected, "encoding of {}", packet.name());

    let mut input = expected;
    let decoded = Packet::decode(packet.packet_id(), &mut input).unwrap();
    assert!(input.is_empty(), "{} left bytes unread", packet.name());
    assert_eq!(decoded, packet);
}

fn request_network_settings(version: i32) -> Packet {
    Packet::RequestNetworkSettings(RequestNetworkSettingsPacket {
        payload: RequestNetworkSettingsPacketPayload { client_network_version: version },
    })
}

fn play_status(status: PlayStatus) -> Packet {
    Packet::PlayStatus(PlayStatusPacket { payload: PlayStatusPacketPayload { status } })
}

#[test]
fn request_network_settings_is_big_endian() {
    assert_eq!(RequestNetworkSettingsPacket::ID, 193);
    // 2259 = 0x000008d3, most significant byte first.
    check(request_network_settings(2259), &[0x00, 0x00, 0x08, 0xd3]);
}

#[test]
fn network_settings() {
    assert_eq!(NetworkSettingsPacket::ID, 143);
    let packet = Packet::NetworkSettings(NetworkSettingsPacket {
        payload: NetworkSettingsPacketPayload {
            compression_threshold: 256,
            compression_algorithm: PacketCompressionAlgorithm::Snappy,
            client_throttle_enabled: true,
            client_throttle_threshold: 8,
            client_throttle_scalar: 0.5,
            sctp_recovery_profile: SctpRecoveryProfile::Default,
        },
    });
    check(
        packet,
        &[
            0x00, 0x01, // compression threshold: u16 LE
            0x01, 0x00, // algorithm: enum as u16 LE
            0x01, // throttle enabled
            0x08, // throttle threshold: u8
            0x00, 0x00, 0x00, 0x3f, // throttle scalar: f32 LE
            0x00, // SCTP recovery profile: enum as u8
        ],
    );
}

#[test]
fn login_carries_the_connection_request_envelope() {
    assert_eq!(LoginPacket::ID, 1);
    let chain_json = r#"{"chain":["a.b.c"]}"#;
    assert_eq!(chain_json.len(), 19);
    let request = ConnectionRequest::from_chain(&["a.b.c"], "h.p.s");
    assert_eq!(request.auth_info, chain_json);

    let packet = Packet::Login(LoginPacket {
        payload: LoginPacketPayload { client_network_version: 2259, connection_request: request },
    });
    let expected = [
        &[0x00, 0x00, 0x08, 0xd3][..], // protocol version: i32 BE
        &[32],                         // envelope length: VarUInt32 (4 + 19 + 4 + 5)
        &[19, 0, 0, 0],                // authentication document length: u32 LE
        chain_json.as_bytes(),
        &[5, 0, 0, 0], // client data token length: u32 LE
        b"h.p.s",
    ]
    .concat();
    check(packet.clone(), &expected);

    // The helpers read the documents straight out of the decoded packet.
    let Packet::Login(login) = Packet::decode(LoginPacket::ID, &mut expected.as_slice()).unwrap() else {
        panic!("not a login packet");
    };
    assert_eq!(login.payload.connection_request.chain().unwrap(), ["a.b.c"]);
    assert_eq!(login.payload.connection_request.client_data_jwt().unwrap(), "h.p.s");
}

#[test]
fn login_survives_length_prefixes_that_are_not_utf8() {
    // A 200-byte document: its length prefix starts with 0xc8, which is not
    // valid UTF-8. Decoding the envelope as text would fail here.
    let request = ConnectionRequest::new("x".repeat(200), "y".repeat(300));
    let packet = Packet::Login(LoginPacket {
        payload: LoginPacketPayload { client_network_version: 2259, connection_request: request },
    });
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    assert_eq!(&bytes[4..10], [0xfc, 0x03, 0xc8, 0x00, 0x00, 0x00]);
    assert_eq!(Packet::decode(LoginPacket::ID, &mut bytes.as_slice()).unwrap(), packet);
}

#[test]
fn play_status_is_big_endian() {
    assert_eq!(PlayStatusPacket::ID, 2);
    check(play_status(PlayStatus::LoginSuccess), &[0, 0, 0, 0]);
    check(play_status(PlayStatus::PlayerSpawn), &[0, 0, 0, 3]);
}

#[test]
fn disconnect_without_message() {
    assert_eq!(DisconnectPacket::ID, 5);
    let packet = Packet::Disconnect(DisconnectPacket {
        payload: DisconnectPacketPayload {
            reason: DisconnectFailReason::NoPermissions,
            messages: DisconnectPacketPayloadMessages::Null,
        },
    });
    check(
        packet,
        &[
            0x04, // reason 2 as ZigZag VarInt32
            0x01, // variant index 1: no message (reads as "skip message = true")
        ],
    );
}

#[test]
fn disconnect_with_messages() {
    let packet = Packet::Disconnect(DisconnectPacket {
        payload: DisconnectPacketPayload {
            reason: DisconnectFailReason::NoPermissions,
            messages: DisconnectPacketPayloadMessages::DisconnectPacketMessages(DisconnectPacketMessages {
                message: "bye".into(),
                filtered_message: "b*e".into(),
            }),
        },
    });
    check(
        packet,
        &[
            0x04, // reason
            0x00, // variant index 0: messages follow
            3, b'b', b'y', b'e', // message
            3, b'b', b'*', b'e', // filtered message
        ],
    );
}

#[test]
fn variant_branches_with_a_constant_write_index_then_constant() {
    // One score entry of kind "change entity": the branch index, then the
    // constant action (as its lower-case name), then the fields.
    let entry = SetScorePacketPayloadScoreInfo::ChangeEntity(ChangeEntityScore {
        scoreboard_id: ScoreboardId { scoreboard_id: 5 },
        objective_name: "obj".into(),
        score_value: 7,
        actor_id: ActorUniqueID { actor_unique_id: -1 },
    });
    let packet = Packet::SetScore(SetScorePacket { payload: SetScorePacketPayload { score_info: vec![entry] } });
    let expected = [
        &[1][..],         // entry count (the list has a default: no presence flag)
        &[2],             // variant index of ChangeEntity
        &[12],            // length of the action name
        b"changeentity",  // constant action
        &[0x0a],          // scoreboard id 5 as ZigZag VarInt64
        &[3],
        b"obj",           // objective name
        &[7, 0, 0, 0],    // score: i32 LE
        &[0x01],          // actor id -1 as ZigZag VarInt64
    ]
    .concat();
    check(packet, &expected);

    // A constant that does not match its branch is rejected.
    let mut wrong = expected.clone();
    wrong[3..15].copy_from_slice(b"changeplayer");
    assert!(matches!(
        Packet::decode(SetScorePacket::ID, &mut wrong.as_slice()),
        Err(ProtocolError::Packet { source, .. }) if matches!(*source, ProtocolError::UnexpectedTag { .. })
    ));
}

#[test]
fn strings_carry_arbitrary_bytes() {
    let blob = BedrockString::from(vec![0xff, 0x00, 0x80]);
    let packet = Packet::Disconnect(DisconnectPacket {
        payload: DisconnectPacketPayload {
            reason: DisconnectFailReason::Unknown,
            messages: DisconnectPacketPayloadMessages::DisconnectPacketMessages(DisconnectPacketMessages {
                message: blob.clone(),
                filtered_message: BedrockString::new(),
            }),
        },
    });
    check(packet, &[0x00, 0x00, 3, 0xff, 0x00, 0x80, 0]);
    assert_eq!(blob.as_str(), None);
}

#[test]
fn framed_packet() {
    let mut bytes = Vec::new();
    encode_framed(&request_network_settings(2259), &mut bytes).unwrap();
    assert_eq!(
        bytes,
        [
            6, // frame length
            0xc1, 0x01, // header: packet id 193 as VarUInt32
            0x00, 0x00, 0x08, 0xd3, // body
        ]
    );
    let mut input = bytes.as_slice();
    let (header, packet) = decode_framed(&mut input).unwrap();
    assert!(input.is_empty());
    assert_eq!(header, PacketHeader::new(193));
    assert_eq!(packet, request_network_settings(2259));
}

#[test]
fn batch_of_consecutive_packets() {
    let packets = [
        request_network_settings(2259),
        play_status(PlayStatus::PlayerSpawn),
        Packet::Disconnect(DisconnectPacket {
            payload: DisconnectPacketPayload {
                reason: DisconnectFailReason::NoPermissions,
                messages: DisconnectPacketPayloadMessages::Null,
            },
        }),
    ];
    let mut batch = Vec::new();
    encode_batch(&packets, &mut batch).unwrap();
    assert_eq!(
        batch,
        [
            6, 0xc1, 0x01, 0x00, 0x00, 0x08, 0xd3, // RequestNetworkSettings
            5, 0x02, 0x00, 0x00, 0x00, 0x03, // PlayStatus
            3, 0x05, 0x04, 0x01, // Disconnect
        ]
    );

    let decoded = decode_batch(&batch).unwrap();
    assert_eq!(decoded.iter().map(|(header, _)| header.packet_id).collect::<Vec<_>>(), [193, 2, 5]);
    assert!(decoded.iter().map(|(_, packet)| packet).eq(packets.iter()));

    // Frames can also be walked without decoding the bodies.
    let bodies: Vec<&[u8]> = frames(&batch).map(|frame| frame.unwrap().body).collect();
    assert_eq!(bodies, [&[0x00, 0x00, 0x08, 0xd3][..], &[0x00, 0x00, 0x00, 0x03], &[0x04, 0x01]]);
}

#[test]
fn sub_client_ids_travel_in_the_header() {
    let header = PacketHeader { packet_id: 2, sender_sub_client: 1, target_sub_client: 2 };
    let mut bytes = Vec::new();
    mcbrs_protocol::codec::encode_framed_with(header, &play_status(PlayStatus::LoginSuccess), &mut bytes).unwrap();
    // 2 | 1 << 10 | 2 << 12 = 0x2402 -> VarUInt32 82 48.
    assert_eq!(bytes, [6, 0x82, 0x48, 0, 0, 0, 0]);
    assert_eq!(decode_framed(&mut bytes.as_slice()).unwrap().0, header);
}

#[test]
fn documented_limits_can_be_validated() {
    // Mojang documents the version field as exactly the protocol version.
    assert_eq!(request_network_settings(PROTOCOL_VERSION).validate(), Ok(()));
    let error = request_network_settings(PROTOCOL_VERSION - 1).validate().unwrap_err();
    let ProtocolError::Packet { name, source } = error else { panic!("{error:?}") };
    assert_eq!(name, "RequestNetworkSettingsPacket");
    assert!(matches!(
        *source,
        ProtocolError::ConstraintViolated { field: "ClientNetworkVersion", constraint, .. }
            if constraint.starts_with("minimum")
    ));
    // Limits are never applied by the codec itself: the packet still encodes.
    assert!(BedrockCodec::to_bytes(&RequestNetworkSettingsPacket {
        payload: RequestNetworkSettingsPacketPayload { client_network_version: 1 }
    })
    .is_ok());
}

#[test]
fn generated_types_convert_to_codec_primitives() {
    use mcbrs_protocol::codec;

    let position = Vec3 { x: 1.0, y: 2.0, z: 3.0 };
    let primitive = codec::Vec3::from(position.clone());
    assert_eq!(primitive, codec::Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(Vec3::from(primitive), position);
    // Same bytes either way.
    assert_eq!(position.to_bytes().unwrap(), primitive.to_bytes().unwrap());

    let uuid: Uuid = "00112233-4455-6677-8899-aabbccddeeff".parse().unwrap();
    let generated = MceUuid::from(uuid);
    assert_eq!(generated.most_significant_bits, 0x0011_2233_4455_6677);
    assert_eq!(generated.least_significant_bits, 0x8899_aabb_ccdd_eeff);
    assert_eq!(generated.to_bytes().unwrap(), uuid.to_bytes().unwrap());
    assert_eq!(Uuid::from(generated), uuid);

    let block = BlockPos { x: -1, y: 64, z: 1 };
    assert_eq!(block.to_bytes().unwrap(), codec::BlockPos::from(block.clone()).to_bytes().unwrap());
    let chunk = ChunkPos { x: -3, z: 9 };
    assert_eq!(chunk.to_bytes().unwrap(), codec::ChunkPos::from(chunk.clone()).to_bytes().unwrap());
    let sub_chunk = SubChunkPos { subchunk_position_x: 1, subchunk_position_y: -2, subchunk_position_z: 3 };
    assert_eq!(sub_chunk.to_bytes().unwrap(), codec::SubChunkPos::from(sub_chunk.clone()).to_bytes().unwrap());
    let rotation = Vec2 { x: 0.5, y: -0.5 };
    assert_eq!(rotation.to_bytes().unwrap(), codec::Vec2::from(rotation.clone()).to_bytes().unwrap());
}
