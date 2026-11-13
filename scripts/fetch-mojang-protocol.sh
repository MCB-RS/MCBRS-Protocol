#!/usr/bin/env bash
# Downloads the `json/` folder of Mojang/bedrock-protocol-docs into
# vendor/bedrock-protocol-docs/json/ and records the upstream commit in
# .mojang-protocol-sha.
#
# Usage: scripts/fetch-mojang-protocol.sh [ref]
#   ref: branch, tag or full commit SHA (default: main)
set -euo pipefail

REPO_URL="https://github.com/Mojang/bedrock-protocol-docs.git"
REF="${1:-main}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$ROOT/vendor/bedrock-protocol-docs"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Sparse, blob-less, depth-1 fetch: only the files of json/ are transferred.
git init --quiet "$TMP/src"
git -C "$TMP/src" config core.longpaths true
git -C "$TMP/src" remote add origin "$REPO_URL"
git -C "$TMP/src" sparse-checkout set --no-cone '/json/*' '/LICENSE*'
git -C "$TMP/src" fetch --quiet --depth 1 --filter=blob:none origin "$REF"
git -C "$TMP/src" checkout --quiet FETCH_HEAD
SHA="$(git -C "$TMP/src" rev-parse HEAD)"

if [ ! -d "$TMP/src/json" ]; then
    echo "error: $REPO_URL has no json/ directory at $REF" >&2
    exit 1
fi
COUNT="$(find "$TMP/src/json" -type f -name '*.json' | wc -l | tr -d ' ')"
if [ "$COUNT" -eq 0 ]; then
    echo "error: json/ is empty at $REF" >&2
    exit 1
fi

# Replace the vendored copy wholesale so files deleted upstream disappear.
rm -rf "$DEST/json"
mkdir -p "$DEST"
cp -R "$TMP/src/json" "$DEST/json"
for license in "$TMP/src"/LICENSE*; do
    if [ -e "$license" ]; then
        cp "$license" "$DEST/"
    fi
done

printf '%s\n' "$SHA" > "$ROOT/.mojang-protocol-sha"
echo "Fetched $COUNT JSON files from $REPO_URL@$SHA"
