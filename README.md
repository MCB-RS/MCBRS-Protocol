# MCBRS-Protocol

Minecraft: Bedrock Edition network protocol for [MCBE-RS](https://github.com/MCB-RS), as a Rust crate.

Every packet, type and enum is **generated** from the JSON schemas Mojang publishes in
[`Mojang/bedrock-protocol-docs`](https://github.com/Mojang/bedrock-protocol-docs) (branch `main`, folder `json/`).
Nothing in `src/generated/` is written or edited by hand.

```rust
use mcbrs_protocol::codec::{decode_batch, encode_batch, Validate};
use mcbrs_protocol::Packet;

// A decompressed batch received from the transport: every packet in it.
for (header, packet) in decode_batch(&batch)? {
    packet.validate()?; // optional: Mojang's documented limits
    handle(header.sender_sub_client, packet);
}

// And the way back.
let mut out = Vec::new();
encode_batch(&[reply], &mut out)?;
```

## Layout

| Path | Content |
| --- | --- |
| `src/codec/` | Hand-written: `BedrockCodec`, `ProtocolError`, wire primitives, NBT, framing, login envelope |
| `src/generated/` | Generated: `enums.rs`, `types.rs`, `packets.rs`, `mod.rs` (+ test-only `samples.rs`) |
| `proto-codegen/` | The generator (`cargo run --bin proto-codegen`) |
| `vendor/bedrock-protocol-docs/json/` | Mojang's schemas, as downloaded |
| `.mojang-protocol-sha` | Commit of Mojang's `main` the schemas come from |
| `scripts/fetch-mojang-protocol.sh` | Downloads `json/` and records the commit |
| `scripts/audit_schema.py` | Exhaustive inventory of the meta-schema |
| `tests/wire_vectors.rs` | Byte-exact vectors for the handshake packets and the framing |
| `.github/workflows/sync-mojang-protocol.yml` | Checks Mojang's `main` every 6 hours and opens a PR |
| `.github/workflows/ci.yml` | On every PR: generated code up to date, check, clippy, tests, docs |

## Updating the protocol

```sh
scripts/fetch-mojang-protocol.sh      # json/ + .mojang-protocol-sha
cargo run --bin proto-codegen         # rewrites src/generated/
cargo check && cargo test
```

`cargo run --bin proto-codegen -- --check` verifies that `src/generated/` matches the schemas without
writing anything. The GitHub workflow runs the three commands above whenever Mojang's `main` moves.

## Codec

`BedrockCodec` (`serialize` / `deserialize`) works on any `bytes::BufMut` / `bytes::Buf`, so `Vec<u8>`,
`&[u8]`, `BytesMut` and `Bytes` all work. Errors are reported through `ProtocolError`. Decoding never
panics, and a length prefix sent by a peer never reserves more than 64 KiB up front.

| Type | Wire |
| --- | --- |
| `u8` ... `i64`, `f32`, `f64`, `bool` | fixed width, little-endian |
| `VarUInt32`, `VarUInt64` | LEB128 |
| `VarInt32`, `VarInt64` | ZigZag, then LEB128 |
| `BedrockString` | `VarUInt32` byte count, then the bytes |
| `Uuid` | most significant 64 bits, then least significant, each as `u64` LE |
| `Vec2`, `Vec3` | `f32` LE components |
| `BlockPos` | three `VarInt32` |
| `ChunkPos` | two `VarInt32` |
| `SubChunkPos` | three `i32` LE |
| `NetworkBlockPos` | `VarInt32`, `VarUInt32`, `VarInt32` (legacy layout, unused by the current schemas) |
| `NetworkNbt` / `Nbt` | NBT in the network (varint) / little-endian flavour |
| `ConnectionRequest` | see below |
| `PacketHeader` | `VarUInt32`: packet id (10 bits), sender and target sub-client (2 bits each) |

`Uuid` converts to and from `u128`, `[u8; 16]`, its two 64-bit halves and the canonical string
(`"…".parse::<Uuid>()`, `to_string()`).

### Strings are bytes

Mojang's schemas say `string` for text **and** for binary payloads: chunk data, NBT blobs, images,
signed tokens. Nothing distinguishes them, so every schema `string` is a `BedrockString`: the bytes
are kept as they are, decoding never fails on content that is not UTF-8, and re-encoding is exact.
Use `as_str()` (or `to_string_lossy()`) when a field is text; `"text".into()` builds one.

### Login connection request

`LoginPacket` and `SubClientLoginPacket` carry a `ConnectionRequest`. The schema types it as a
`string`; it is an envelope of two length-prefixed documents:

```text
VarUInt32  length of what follows
  u32 LE   length of the authentication document
  bytes    authentication document (JSON)
  u32 LE   length of the client data token
  bytes    client data token (JWT)
```

Both documents are stored as raw bytes. `chain()` returns the certificate chain from either layout
of the authentication document (`{"chain": [...]}` or `{"AuthenticationType", "Certificate", "Token"}`),
`client_data_jwt()` the client token, `authentication_type()` and `token()` the remaining fields.
Verifying the signatures is out of scope.

### Framing and batches

```text
frame = VarUInt32 length, then: VarUInt32 packet header, packet body
batch = frame, frame, ...
```

| Function | Purpose |
| --- | --- |
| `encode_framed` / `encode_framed_with` | one packet as a frame (default header / explicit sub-client ids) |
| `decode_framed` | one frame off the front of a buffer |
| `encode_batch` / `decode_batch` | all the packets of a batch |
| `frames` / `split_frame` | walk a batch without decoding bodies: route, filter, skip unknown packets |
| `put_batch_envelope` / `get_batch_envelope` | the RakNet envelope: `0xfe`, then the compression byte (`0x00` deflate, `0x01` snappy, `0xff` none) once compression is negotiated |

A frame carries its own length, so a packet this crate cannot decode never desynchronizes the rest
of the batch. A body longer than its packet is an error, not ignored.

Compression itself is not implemented here: the algorithm, the threshold and above all the
decompression limits are transport policy. Decompress, then hand the result to `decode_batch`.

### Validation

Mojang documents limits (`maxLength`, `minItems` / `maxItems`, `minimum` / `maximum`,
`maxProperties`). They do not affect the layout and the codec never applies them. Every generated
type implements `Validate`; `validate()` checks a value and everything inside it and reports the
first violation as `ProtocolError::ConstraintViolated { type_name, field, constraint }`. The limits
are also listed in each field's documentation.

## Translation rules

The schemas are JSON Schema (draft-07) with `x-*` extensions; `scripts/audit_schema.py` lists every
key, value and node shape, and the generator's parser rejects anything outside that inventory.

They state structure and encoding hints, not a wire grammar. The rules below were checked against
[gophertunnel](https://github.com/Sandertv/gophertunnel) (protocol 2225, one release behind the
schemas) by comparing its hand-written marshalling of the packets named in the last column.

| Schema | Rust | Wire | Checked against |
| --- | --- | --- | --- |
| `object` | `struct`, fields in `x-ordinal-index` order | fields in order | every packet below |
| not in `required`, no `default` | `Option<T>` | `bool`, then the value | Animate, Interact, MovePlayer, PlayerAuthInput, MoveActorDelta, LevelChunk, SubChunk |
| not in `required`, with `default` | `T` | the value, always | SetScore, PlayerList, CommandOrigin, CommandOutput, Text |
| `integer` | `u8` ... `i64` | fixed LE; `Compression` → varint; `Big Endian` | Login, PlayStatus, NetworkSettings |
| `string` | `BedrockString` | `VarUInt32` count + bytes | Login, LevelChunk, ServerToClientHandshake |
| `array` | `Vec<T>` | `VarUInt32` count + elements | SetHud, PlayerAuthInput |
| `array` + `No size compression` | `Vec<T>` | `u32` LE count + elements | NetworkChunkPublisherUpdate |
| `additionalProperties` | `Vec<(K, V)>` | `VarUInt32` count + pairs | DimensionData, GraphicsOverrideParameter |
| enum + `Enum-as-Value` | Rust `enum` | its number, in the use-site integer encoding | NetworkSettings, PlayStatus, MovePlayer |
| enum without `Enum-as-Value` | Rust `enum` | its name in lower case, as a string | Animate, CommandOrigin, CommandOutput, ResourcePackClientResponse |
| `oneOf` | Rust `enum` | `VarUInt32` branch index, then the branch | Disconnect, SetScore, ResourcePackClientResponse |
| `const` property | associated constant | written like any field; checked when reading | SetScore, ResourcePackClientResponse, entity metadata |
| no `type` / `$ref` / `oneOf` | `NetworkNbt` | network NBT | BlockActorData |
| `Allow unknown enum values` | `MaybeUnknown<E>` | same as the enum | not checked |
| `array`, `minItems == maxItems` | `[T; N]` | elements only | not checked |
| type that only refers to itself | `BedrockString` | string | ServerToClientHandshake (`WebToken`) |

Notes on what the comparison settled:

- **Presence conditions stated in prose are optionals.** Mojang describes about thirty fields as
  "present only when…" (`MovePlayer.TeleportData`, the `PlayerAuthInput` item and vehicle fields, the
  `MoveActorDelta` components). On the wire they are ordinary `std::optional`s: the presence byte is
  always there. `scripts/audit_schema.py --optional` lists all 261 properties missing from
  `required`, split into the 207 optionals and the 54 defaulted members.
- **`DisconnectPacket`'s "skip message" flag is the variant index.** The schema models the messages
  as `oneOf [messages, null]`; index 1 (one byte, `01`) is "no message".
- **Flag sets are lists.** `PlayerAuthInput`'s input data is a count followed by the ids of the set
  flags. No enum in the schemas is a bit mask, so no bit-flag helpers are generated.

### The one place the schema is not enough

`proto-codegen/src/overrides.rs` lists the properties whose schema type under-describes the wire.
There are two, both for `ConnectionRequest` (above). The generator fails if an entry stops matching
the schemas, so the list cannot go stale silently.

### Other points

- **Identifiers**: C++ names (`::`, `<>`, spaces, punctuation) become `PascalCase` types / variants and
  `snake_case` fields; Rust keywords are escaped (`r#type`, `self_`), leading digits get a `_`, and
  collisions are numbered.
- **Enum aliases**: C++ enumerators sharing a value cannot be Rust discriminants. The first one is the
  variant, the others are associated constants (`GameType::WorldDefault`). Enums carry no `#[repr]`;
  `value()`, `from_value()`, `From` / `TryFrom` and `ProtocolEnum` do the conversions.
- **Recursive types**: a dependency graph over all types is split into strongly connected components
  (Tarjan); by-value fields inside a component are wrapped in `Box<T>`. The current protocol has none.
- **Primitives**: the generated `MceUuid`, `Vec2`, `Vec3`, `BlockPos`, `ChunkPos` and `SubChunkPos`
  convert to and from their `codec` counterparts with `From`. The generator checks that each still
  has the layout of its counterpart and fails otherwise.
- **Packets**: `Packet` has one variant per packet with `packet_id()`, `name()`, `encode()`,
  `decode(packet_id, buf)`, `validate()`, plus header-aware `encode_with_header()` / `decode_with_header()`.

## Tests

`cargo test` covers:

- the primitives, NBT, strings, the login envelope and the framing, with known byte sequences and
  malformed input;
- the generator: naming, lowering, constraints, overrides, cycle breaking;
- every generated packet: a round trip of its default value, round trips of 16 pseudo-random values
  (including strings that are not UTF-8), and decoding of truncated input;
- `tests/wire_vectors.rs`: the exact bytes of `RequestNetworkSettings`, `NetworkSettings`, `Login`,
  `PlayStatus`, `Disconnect` (with and without messages), a `SetScore` entry, a framed packet and a
  batch.

## What is and is not verified

The byte vectors are written by hand from the documented layout and cross-checked against
gophertunnel's code. They are **not** captures of a live session, and gophertunnel is one protocol
release behind the schemas. Rules marked "not checked" above, and every packet not named in the
table, rest on the schema alone. Confirm against real traffic before relying on a given packet.

## Licensing

The crate is MIT licensed. The files under `vendor/bedrock-protocol-docs/` are (c) Mojang AB and remain
subject to their own license (see `vendor/bedrock-protocol-docs/LICENSE`).
