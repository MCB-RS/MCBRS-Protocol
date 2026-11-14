## 📝 Summary

<!-- Describe what this PR changes and why. Focus on the changes made in `src/codec/` or `proto-codegen/` rather than the auto-generated diff in `src/generated/`. -->

Fixes #<!-- issue number, if applicable -->

---

## 🔧 Type of Change

- [ ] **Wire-Fidelity Fix (`proto-codegen` / `overrides.rs`):** Fixes packet or type serialization to match the real Bedrock wire format.
- [ ] **Codec Primitive (`src/codec/`):** Adds or updates hand-written binary primitives (VarInt, NBT, strings, framing, `ConnectionRequest`).
- [ ] **Schema Update (`.mojang-protocol-sha`):** Updates the pinned `Mojang/bedrock-protocol-docs` commit and regenerates `src/generated/`.
- [ ] **Tests / Benchmarks (`tests/`):** Adds wire-vector tests, round-trip checks, or fuzzing/property cases.
- [ ] **Documentation / CI / Tooling:** Updates docs, scripts, or GitHub Actions workflows.

---

## 🔬 Wire Evidence & Verification

<!-- If this PR changes how any packet or type is encoded/decoded on the wire, provide the evidence below (e.g., raw hex capture from a real Bedrock client/server session or the exact schema rule). If not applicable, write "N/A". -->

- **Affected Packet(s) / Type(s):** ``
- **Raw Hex Payload / Wire Vector:**
  ```text
  
  ```

---

## ✅ Pre-Merge Checklist

- [ ] **No manual edits in `src/generated/`:** All changes to `src/generated/` were produced by running `cargo run --bin proto-codegen` (`cargo run --bin proto-codegen -- --check` passes).
- [ ] **EULA Compliance:** No files under `vendor/` or proprietary Mojang schemas/assets are staged or committed.
- [ ] **DoS & Panic Safety:** No `unwrap()`, `expect()`, or unbounded allocations were introduced in untrusted decoding paths (`src/codec/`).
- [ ] **Regression Tests:** Added or updated byte-level tests in `tests/wire_vectors.rs` (if fixing wire serialization).
- [ ] **Quality Gates Pass Locally:**
  - [ ] `cargo fmt --all -- --check`
  - [ ] `cargo clippy --workspace --all-targets -- -D warnings`
  - [ ] `cargo test --workspace`
  - [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`
