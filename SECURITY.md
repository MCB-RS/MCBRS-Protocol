# Security Policy

**MCBRS-Protocol** (`mcbrs-protocol`) sits directly on the untrusted network boundary of a Minecraft: Bedrock Edition server or client, deserializing raw binary frames and NBT payloads before any game logic runs. We treat any input-triggered crash, unbounded allocation, or infinite loop in the codec layer as a security vulnerability.

---

## 📌 Supported Versions

Because Minecraft: Bedrock Edition is a rolling-release game, security fixes are applied to the latest protocol version tracked on the `main` branch.

---

## 🎯 Scope & Threat Model

All functions in `mcbrs-protocol` that consume untrusted bytes (`BedrockCodec::decode`, `decode_batch`, `decode_framed`, `NbtTag` readers, and `Validate::validate`) must safely handle arbitrary, truncated, or maliciously crafted byte slices (`&[u8]`).

### In Scope (Security Vulnerabilities)
Any crafted byte sequence that causes `mcbrs-protocol` to exhibit the following behaviors during decoding or validation is **in scope**:
1. **Panics or Aborts (1-Packet DoS):** Any `unwrap()`, `expect()`, out-of-bounds slice index, or arithmetic overflow/underflow triggered by untrusted network bytes.
2. **Unbounded Memory Allocation (OOM Bombs):** A small payload declaring a massive `VarUInt32` length prefix (for strings, byte arrays, packet batches, or vectors/maps) that causes the decoder to pre-allocate gigabytes of RAM via `Vec::with_capacity` without bounding it against remaining input bytes or protocol limits.
3. **Stack Overflow via Deep Recursion:** Crafted nested structures (such as deeply nested Network NBT compounds/lists or recursive `oneOf` types) that exhaust the thread call stack.
4. **CPU Exhaustion / Infinite Loops:** Malformed input that causes a decoder loop to spin indefinitely without advancing the cursor or returning a `CodecError`.
5. **Authentication Token Bypass (`ConnectionRequest`):** Flaws in `src/codec/connection.rs` that allow malformed JWT chains or client data tokens to cause panics or bypass structural validation.

### Out of Scope
- **Transport-layer vulnerabilities** (WebRTC, DTLS, SCTP, HTTP `/v1/join` signaling, or `a=identity` JWS verification): Please report these on the [**`MCBRS-NetherNet`**](https://github.com/MCB-RS/MCBRS-NetherNet/security) repository instead.
- **Build-time CLI (`proto-codegen`) with corrupted local JSON files:** `proto-codegen` is a build-time tool intended to run on trusted developer and CI environments against pinned schemas, not on untrusted runtime traffic.
- **Vulnerabilities in the official Minecraft Bedrock client or Bedrock Dedicated Server:** Please report vulnerabilities affecting official Mojang/Microsoft software directly to the [Microsoft Security Response Center (MSRC)](https://msrc.microsoft.com/).

---

## 🚨 Reporting a Vulnerability

**Please do NOT report security vulnerabilities through public GitHub issues, discussions, or pull requests.** Publicly posting a 1-packet crash payload puts downstream servers at immediate risk.

### Preferred Channel: GitHub Private Vulnerability Reporting
Please submit your report privately via GitHub Security Advisories:
👉 [**Report a vulnerability on MCBRS-Protocol**](https://github.com/MCB-RS/MCBRS-Protocol/security/advisories/new)

*(If you are unable to use GitHub Security Advisories, you may contact the organization maintainer directly via the email address listed on the [MCB-RS GitHub profile](https://github.com/MCB-RS)).*

### What to Include in Your Report
To help us reproduce and patch the issue quickly, please include:
1. **Affected Component:** The specific packet name, generated type, or codec module (`src/codec/io.rs`, `src/codec/nbt.rs`, `src/codec/framing.rs`, etc.).
2. **Minimal Reproducer:** A minimal Rust snippet or raw hex byte slice (`&[0x..., ...]`) demonstrating the crash, OOM, or hang:
   ```rust
   #[test]
   fn repro_crash() {
       let payload: &[u8] = &[/* crafted bytes */];
       let _ = mcbrs_protocol::Packet::decode_batch(payload);
   }
   ```
3. **Impact:** Panic backtrace, allocation size requested, or stack overflow details.
4. **Commit SHA:** The commit of `MCBRS-Protocol` (and `.mojang-protocol-sha`) tested.

---

## ⏱️ Disclosure & Response Process

1. **Acknowledgment:** We aim to acknowledge receipt of your report within **48 to 72 hours**.
2. **Fix & Regression Test:** Once confirmed, we will develop a fix in `src/codec/` or `proto-codegen/`, regenerate `src/generated/`, and add the triggering byte sequence to our regression test suite.
3. **Coordinated Disclosure:** Once the patch is merged and published, we will publish the security advisory and credit you (unless you prefer to remain anonymous).

---

## ⚖️ Legal Notice

**NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR MICROSOFT.**
