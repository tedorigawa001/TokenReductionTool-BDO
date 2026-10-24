#!/bin/sh
# Tampered copies of the live formula must fail check-homebrew-tap.sh.
# Needs the same tools as the check (curl, gh) and network access.

set -eu

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
CHECK="$REPO_ROOT/scripts/check-homebrew-tap.sh"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

curl -fsSL "https://raw.githubusercontent.com/tedorigawa001/homebrew-tap/HEAD/Formula/bushido.rb" \
    -o "$WORK/live.rb"

FAIL=0
expect_fail() { # description, formula
    if sh "$CHECK" "$2" > "$WORK/out" 2>&1; then
        echo "  FAIL: $1 — check passed a tampered formula"
        sed 's/^/      /' "$WORK/out"
        FAIL=1
    else
        echo "  PASS: $1 — $(grep -m1 'FAIL:' "$WORK/out" | sed 's/^ *FAIL: //')"
    fi
}

echo "==> Tampered formulas are rejected"

# First url only, so each case exercises one archive.
FIRST_URL=$(sed -n 's/^[[:space:]]*url "\(.*\)"$/\1/p' "$WORK/live.rb" | head -n 1)
FIRST_SHA=$(sed -n 's/^[[:space:]]*sha256 "\(.*\)"$/\1/p' "$WORK/live.rb" | head -n 1)

# An attacker's host with its own (self-consistent) sha256.
sed "s|$FIRST_URL|https://evil.example/bushido-aarch64-apple-darwin.tar.xz|" "$WORK/live.rb" > "$WORK/host.rb"
expect_fail "url moved to another host" "$WORK/host.rb"

# Same repository, but another project's or a fork's release path.
sed "s|tedorigawa001/TokenReductionTool-BDO/releases|attacker/TokenReductionTool-BDO/releases|" "$WORK/live.rb" > "$WORK/fork.rb"
expect_fail "url moved to a fork's releases" "$WORK/fork.rb"

# Right url, binary swapped: the sha256 no longer matches the asset.
REST=${FIRST_SHA#?}
if [ "${FIRST_SHA%"$REST"}" = 0 ]; then BAD_SHA="1$REST"; else BAD_SHA="0$REST"; fi
sed "s|$FIRST_SHA|$BAD_SHA|" "$WORK/live.rb" > "$WORK/sha.rb"
expect_fail "sha256 not matching the release asset" "$WORK/sha.rb"

# Rolled back to an older, pre-attestation release (v0.47.0): version lag and
# missing provenance must both fail.
sed -E 's/(version ")[^"]+"/\10.47.0"/; s|/download/v[0-9.]+/|/download/v0.47.0/|' "$WORK/live.rb" > "$WORK/old.rb"
expect_fail "rolled back to an unattested release" "$WORK/old.rb"

echo ""
if [ "$FAIL" -eq 0 ]; then
    echo "All tampered formulas rejected"
else
    echo "check-homebrew-tap.sh accepted a tampered formula" >&2
    exit 1
fi
