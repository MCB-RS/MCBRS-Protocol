# Contributing to MCBRS-Protocol

Thank you for your interest in contributing to **MCBRS-Protocol** (`mcbrs-protocol`)! This repository provides the wire-level serialization codecs and automated schema code generation for the **MCB-RS** ecosystem.

Because most of the packet and type definitions in this crate are generated automatically from upstream protocol schemas, please read this guide carefully before opening an issue or pull request.

---

## 🚨 The 3 Golden Rules

1. **Never edit `src/generated/` by hand.**
   All files under `src/generated/` (`packets.rs`, `types.rs`, `enums.rs`, `samples.rs`) are produced by `proto-codegen`. Any manual edit will be overwritten and rejected by CI (`cargo run --bin proto-codegen -- --check`). If a packet or type is generated incorrectly, fix the generator in `proto-codegen/` (or add a targeted rule in `proto-codegen/src/overrides.rs`) and regenerate.
2. **Never commit `vendor/` or proprietary Mojang files.**
   To strictly comply with the Minecraft EULA, upstream JSON schemas from `Mojang/bedrock-protocol-docs` are **never** tracked in Git or distributed in the crate. They are fetched locally on demand via `scripts/fetch-mojang-protocol.sh` using the commit hash pinned in `.mojang-protocol-sha`.
3. **Strict Clean-Room & Wire-Fidelity Policy.**
   Contributions must be original implementations based on public protocol schemas, real network captures, and clean-room analysis. Never submit decompiled game code or proprietary assets.

---

## 🗂️ Repository Layout

Understanding where to make your changes:

| Path | Description | Editable? |
| :--- | :--- | :---: |
| `src/codec/` | Hand-written binary primitives (`io.rs`, `string.rs`, `nbt.rs`, `primitives.rs`, `connection.rs`, `framing.rs`, `error.rs`). | ✅ **Yes** |
| `proto-codegen/` | The schema parser, intermediate representation (`ir.rs`, `lower.rs`), cycle breaker (`cycles.rs`), overrides (`overrides.rs`), and Rust emitter (`emit.rs`). | ✅ **Yes** |
| `tests/` | Wire-level byte vector tests (`tests/wire_vectors.rs`) and round-trip integration tests. | ✅ **Yes** |
| `scripts/` | Helper scripts to fetch upstream schemas (`fetch-mojang-protocol.sh`) and audit schema properties (`audit_schema.py`). | ✅ **Yes** |
| `.mojang-protocol-sha` | Pinned commit SHA of `Mojang/bedrock-protocol-docs` used for reproducible builds. | ⚠️ **Via script** |
| `src/generated/` | Auto-generated Rust AST for ~300 packets and ~700 types/enums. | ❌ **Auto-generated** |
| `vendor/` | Local, git-ignored working copy of the upstream JSON schemas. | ❌ **Git-ignored** |

---

## 🛠️ Development Setup

### Prerequisites
- **Rust Toolchain:** Rust `1.88+` (2024 Edition).
- **Standard Unix tools:** `bash`, `git`, `curl` / `tar` (available natively on Linux/macOS and via Git Bash / WSL on Windows).

### 1. Clone and Fetch Protocol Schemas
After cloning the repository, fetch the exact upstream JSON schemas pinned in `.mojang-protocol-sha`:

```bash
git clone [https://github.com/MCB-RS/MCBRS-Protocol.git](https://github.com/MCB-RS/MCBRS-Protocol.git)
cd MCBRS-Protocol

# Fetch the pinned Mojang protocol schemas into vendor/bedrock-protocol-docs/json
bash scripts/fetch-mojang-protocol.sh "$(tr -d '[:space:]' < .mojang-protocol-sha)"
```

### 2. Regenerate Code (After Editing `proto-codegen`)
Whenever you modify `proto-codegen/`, regenerate `src/generated/` and verify that everything compiles:

```bash
cargo run --bin proto-codegen
```

To verify that `src/generated/` is strictly in sync with `proto-codegen` without modifying files on disk:

```bash
cargo run --bin proto-codegen -- --check
```

---

## 🔧 Common Contribution Workflows

### Fixing a Wire-Encoding Discrepancy
Upstream JSON schemas occasionally omit legacy C++ serialization quirks (for example, Big-Endian fields in handshake packets or raw string wrappers like `ConnectionRequest`).

If you discover a mismatch between a generated packet and the real Minecraft Bedrock client:
1. Check if it is a general schema rule (in `proto-codegen/src/schema.rs` or `lower.rs`) or a packet-specific exception (in `proto-codegen/src/overrides.rs`).
2. Implement the fix in `proto-codegen/` (and `src/codec/` if a new primitive helper is needed).
3. Run `cargo run --bin proto-codegen` to update `src/generated/`.
4. **Add a regression test** in `tests/wire_vectors.rs` with the exact raw byte sequence proving the wire format.

### Updating to a Newer Protocol Version
Protocol updates are normally handled automatically by our scheduled GitHub Actions workflow (`.github/workflows/sync-mojang-protocol.yml`). To test a protocol update locally:

```bash
# Fetch the latest main commit from Mojang/bedrock-protocol-docs
bash scripts/fetch-mojang-protocol.sh

# Regenerate the Rust code
cargo run --bin proto-codegen

# Run the full test suite
cargo test --workspace
```

---

## ✅ Quality Gates & Pre-PR Checklist

Before submitting a Pull Request, **all** of the following commands must pass locally with **0 errors and 0 warnings**:

```bash
# 1. Verify generated code is up-to-date
cargo run --bin proto-codegen -- --check

# 2. Check formatting
cargo fmt --all -- --check

# 3. Run Clippy across the entire workspace
cargo clippy --workspace --all-targets -- -D warnings

# 4. Run all unit, property, and wire-vector tests
cargo test --workspace

# 5. Verify documentation builds cleanly without broken links
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

# 6. Ensure no files under vendor/ are staged
git status
```

---

## 📝 Pull Request Guidelines

- **Keep PRs focused:** Submit one logical change or fix per pull request.
- **Separate generator changes from generated diffs in review explanation:** In your PR description, clearly explain the changes made in `src/codec/` or `proto-codegen/` so reviewers do not have to hunt through thousands of lines in `src/generated/`.
- **Include wire evidence:** When fixing packet serialization, include the raw hex payload captured from a real Bedrock client/server session or explain the exact protocol specification rule being addressed.
- **Follow Rust idioms:** Avoid `unwrap()` / `expect()` in decoding paths (`src/codec/`). Decoding untrusted network bytes must always return a typed `Result<T, CodecError>` without panicking.

---

## 🤝 Code of Conduct & Licensing

By participating in this project, you agree to abide by our [Code of Conduct](CODE_OF_CONDUCT.md).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in **MCBRS-Protocol** shall be licensed under the [**MIT License**](LICENSE), without any additional terms or conditions.

**NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR MICROSOFT.**
