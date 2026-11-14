#!/usr/bin/env bash
# Downloads the `json/` folder of Mojang/bedrock-protocol-docs into
# vendor/bedrock-protocol-docs/json/ (git-ignored: Mojang's files are never
# committed to this repository) and records the upstream commit in
# .mojang-protocol-sha.
#
# Usage:
#   scripts/fetch-mojang-protocol.sh             latest commit of `main`
#   scripts/fetch-mojang-protocol.sh --latest    same
#   scripts/fetch-mojang-protocol.sh --locked    the commit in .mojang-protocol-sha
#   scripts/fetch-mojang-protocol.sh <sha>       that exact commit
#
# `--locked` (or an explicit SHA) is what makes a build reproducible: it
# fetches precisely the schemas the committed src/generated/ was built from.
set -euo pipefail

REPO_URL="https://github.com/Mojang/bedrock-protocol-docs.git"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$ROOT/vendor/bedrock-protocol-docs"
SHA_FILE="$ROOT/.mojang-protocol-sha"

usage() {
    sed -n '2,14p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

if [ "$#" -gt 1 ]; then
    usage >&2
    exit 2
fi

case "${1:---latest}" in
    -h | --help)
        usage
        exit 0
        ;;
    --latest)
        REF="main"
        EXPECTED=""
        ;;
    --locked)
        if [ ! -s "$SHA_FILE" ]; then
            echo "error: --locked needs a commit in .mojang-protocol-sha" >&2
            exit 1
        fi
        REF="$(tr -d '[:space:]' < "$SHA_FILE")"
        EXPECTED="$REF"
        ;;
    -*)
        echo "error: unknown option $1" >&2
        usage >&2
        exit 2
        ;;
    *)
        REF="$1"
        EXPECTED="$1"
        ;;
esac

# A pinned version must be a full commit SHA: a branch or a tag can move.
if [ -n "$EXPECTED" ] && ! [[ "$EXPECTED" =~ ^[0-9a-f]{40}$ ]]; then
    echo "error: '$EXPECTED' is not a full 40-character commit SHA" >&2
    exit 1
fi

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

if [ -n "$EXPECTED" ] && [ "$SHA" != "$EXPECTED" ]; then
    echo "error: asked for $EXPECTED but fetched $SHA" >&2
    exit 1
fi
if [ ! -d "$TMP/src/json" ]; then
    echo "error: $REPO_URL has no json/ directory at $REF" >&2
    exit 1
fi
COUNT="$(find "$TMP/src/json" -type f -name '*.json' | wc -l | tr -d ' ')"
if [ "$COUNT" -eq 0 ]; then
    echo "error: json/ is empty at $REF" >&2
    exit 1
fi

# Replace the local copy wholesale so files deleted upstream disappear.
rm -rf "$DEST/json"
mkdir -p "$DEST"
cp -R "$TMP/src/json" "$DEST/json"
for license in "$TMP/src"/LICENSE*; do
    if [ -e "$license" ]; then
        cp "$license" "$DEST/"
    fi
done

printf '%s\n' "$SHA" > "$SHA_FILE"
echo "Fetched $COUNT JSON files from $REPO_URL@$SHA"
