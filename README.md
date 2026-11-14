# MCBRS-Protocol

Minecraft: Bedrock Edition network protocol for [MCBE-RS](https://github.com/MCB-RS), as a Rust crate.

Every packet, type and enum is **generated** from the JSON schemas Mojang publishes in
[`Mojang/bedrock-protocol-docs`](https://github.com/Mojang/bedrock-protocol-docs) (branch `main`, folder `json/`).
Nothing in `src/generated/` is written or edited by hand.

```rust
use mcbrs_protocol::{BedrockCodec, Packet};
use mcbrs_protocol::packets::DisconnectPacket;

// Decode a packet body from its official id...
let packet = Packet::decode(DisconnectPacket::ID, &mut bytes)?;
// ...and encode it back.
let mut out = Vec::new();
packet.encode(&mut out)?;
```

## Layout

| Path | Content |
| --- | --- |
| `src/codec/` | Hand-written: `BedrockCodec`, `ProtocolError`, wire primitives, NBT |
| `src/generated/` | Generated: `enums.rs`, `types.rs`, `packets.rs`, `mod.rs` (+ test-only `samples.rs`) |
| `proto-codegen/` | The generator (`cargo run --bin proto-codegen`) |
| `vendor/bedrock-protocol-docs/json/` | Mojang's schemas, as downloaded |
| `.mojang-protocol-sha` | Commit of Mojang's `main` the schemas come from |
| `scripts/fetch-mojang-protocol.sh` | Downloads `json/` and records the commit |
| `scripts/audit_schema.py` | Exhaustive inventory of the meta-schema |
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
`&[u8]`, `BytesMut` and `Bytes` all work. Errors are reported through `ProtocolError`; decoding never
panics and never allocates more than the input can justify.

Primitives: little-endian integers and floats, `bool`, `VarUInt32` / `VarInt32` / `VarUInt64` / `VarInt64`
(ZigZag for the signed ones), `String`, `Uuid`, `Vec2`, `Vec3`, `BlockPos`, `SubChunkPos`, `PacketHeader`,
`Nbt` (little-endian) and `NetworkNbt` (varint).

## What the schemas look like

`scripts/audit_schema.py` walks every file and lists every key, value and node shape. The files are
JSON Schema (draft-07) documents extended with `x-*` keys:

| Key | Values found |
| --- | --- |
| `type` | `object`, `string`, `integer`, `boolean`, `array`, `number`, `null`, or absent |
| `x-underlying-type` | `uint8`, `int8`, `uint16`, `int16`, `uint32`, `int32`, `uint64`, `int64`, `float`, `double`, `boolean`, `object` |
| `x-serialization-options` | `Compression`, `Enum-as-Value`, `Allow unknown enum values`, `No size compression`, `Big Endian` |
| `x-control-value-type` | `uint32` |
| Structure | `properties`, `required`, `x-ordinal-index`, `items`, `minItems`, `maxItems`, `additionalProperties`, `propertyNames`, `oneOf`, `const`, `$ref` |
| Enums | `enum`, `x-enum-binary-value`, `$enumDescription` |
| Packets | `$metaProperties` (`[cereal:packet]`, `[cereal:packet_details]`) |
| Validation only | `minimum`, `maximum`, `minLength`, `maxLength`, `maxProperties`, `pattern`, `default`, `x-runtime-constraint-description` |

The generator's parser is strict: a key or value outside this inventory aborts generation with the file and
path of the offending node, so a new construct can never be silently mistranslated.

## Translation rules

The schemas describe structure and encoding hints, not a formal wire grammar. These are the rules the
generator applies; the ones marked *inferred* are not stated anywhere by Mojang.

| Schema | Rust | Wire |
| --- | --- | --- |
| `object` with `properties` | `struct`, fields in `x-ordinal-index` order | fields in order |
| property missing from `required` | `Option<T>` | `bool` flag, then the value (*inferred*) |
| `integer` | `u8` ... `i64` | fixed little-endian; `Compression` -> varint; `Big Endian` |
| `number` | `f32` / `f64` | little-endian |
| `string` | `String` | `VarUInt32` length + UTF-8 |
| `array` | `Vec<T>` | `VarUInt32` count (`u32` with `No size compression`) |
| `array`, `minItems == maxItems` | `[T; N]` | elements only (*inferred*) |
| `additionalProperties` | `Vec<(K, V)>` | `VarUInt32` count + pairs (*inferred*) |
| enum with `Enum-as-Value` | Rust `enum` | its number, in the use-site integer encoding |
| enum without `Enum-as-Value` | Rust `enum` | its name, lower-cased, as a string (*inferred*) |
| `Allow unknown enum values` | `MaybeUnknown<E>` | same as the enum |
| `oneOf` | Rust `enum` | varint branch index, then the branch (*inferred*) |
| `oneOf` whose branches all start with a `const` of one enum | Rust `enum` | the constant alone discriminates |
| no `type` / `$ref` / `oneOf` | `NetworkNbt` | network NBT (*inferred*) |
| type that only refers to itself (`WebToken`) | `String` | string (*inferred*) |

Other points:

- **Identifiers**: C++ names (`::`, `<>`, spaces, punctuation) become `PascalCase` types / variants and
  `snake_case` fields; Rust keywords are escaped (`r#type`, `self_`), leading digits get a `_`, and
  collisions are numbered.
- **Enum aliases**: C++ enumerators sharing a value cannot be Rust discriminants. The first one is the
  variant, the others are associated constants (`GameType::WorldDefault`). Enums carry no `#[repr]`;
  `value()`, `from_value()`, `From` / `TryFrom` and `ProtocolEnum` do the conversions.
- **Recursive types**: a dependency graph over all types is split into strongly connected components
  (Tarjan); by-value fields inside a component are wrapped in `Box<T>`. The current protocol has none.
- **Packets**: `Packet` has one variant per packet with `packet_id()`, `name()`, `encode()`,
  `decode(packet_id, buf)`, plus header-aware `encode_with_header()` / `decode_with_header()`.

## Tests

`cargo test` covers the primitives with known byte sequences, the generator (naming, lowering, cycle
breaking), and for every generated packet: a round trip of its default value, round trips of 16
pseudo-random values, and decoding of truncated input.

These tests prove that encoding and decoding agree with each other. They cannot prove agreement with the
game: the rules marked *inferred* above should be confirmed against real captures before relying on a
given packet.

## Licensing

The crate is MIT licensed. The files under `vendor/bedrock-protocol-docs/` are (c) Mojang AB and remain
subject to their own license (see `vendor/bedrock-protocol-docs/LICENSE`).
