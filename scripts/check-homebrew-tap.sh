#!/bin/sh
# Verifies the published Homebrew formula still points at genuine release
# artifacts.
#
# The formula pins each archive's sha256, so a replaced GitHub release asset
# already fails `brew install`. What brew cannot catch is the tap itself being
# rewritten — e.g. with a leaked HOMEBREW_TAP_TOKEN, an attacker swaps both the
# url and the sha256 for their own binary. This check, run daily from CI,
# catches that within a day: every url must be this repository's release asset
# for the formula's version, every sha256 must match that asset, and the asset
# must carry build provenance from this repository's release.yml at that tag.
#
# Usage: check-homebrew-tap.sh [formula-file]
#   With no argument the live formula is fetched from the tap.
# Needs curl, sha256sum or shasum, and an authenticated gh (GH_TOKEN in CI).

set -eu

REPO="tedorigawa001/TokenReductionTool-BDO"
TAP_FORMULA_URL="https://raw.githubusercontent.com/tedorigawa001/homebrew-tap/HEAD/Formula/bushido.rb"
CRATE_NAME="bushido"

FAIL=0
pass() { echo "  PASS: $1"; }
fail() { echo "  FAIL: $1"; FAIL=1; }

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

FORMULA="$WORK/bushido.rb"
if [ $# -ge 1 ]; then
    cp "$1" "$FORMULA"
else
    curl -fsSL "$TAP_FORMULA_URL" -o "$FORMULA"
fi

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{ print $1 }'
    else
        shasum -a 256 "$1" | awk '{ print $1 }'
    fi
}

echo "==> Homebrew tap integrity"

VERSION=$(sed -n 's/^[[:space:]]*version "\([^"]*\)"$/\1/p' "$FORMULA" | head -n 1)
case "$VERSION" in
    '' | *[!0-9.]*)
        fail "formula has no plain version (got: '${VERSION}')"
        echo "Homebrew tap integrity REGRESSED" >&2
        exit 1
        ;;
esac
TAG="v${VERSION}"
echo "    formula version: ${VERSION}"

LATEST=$(gh release view --repo "$REPO" --json tagName --jq .tagName 2>/dev/null || true)
if [ -z "$LATEST" ]; then
    fail "could not look up the latest release of ${REPO}"
elif [ "$LATEST" = "$TAG" ]; then
    pass "formula tracks the latest release (${TAG})"
else
    # Not tampering by itself (publish-homebrew-formula may have failed, or a
    # release is mid-flight), but the tap should never lag silently.
    fail "formula is ${TAG}, latest release is ${LATEST}"
fi

# One "url sha256" pair per line, in formula order.
awk '
    /^[[:space:]]*url "/    { u = $2; gsub(/"/, "", u) }
    /^[[:space:]]*sha256 "/ { s = $2; gsub(/"/, "", s); print u, s; u = "" }
' "$FORMULA" > "$WORK/pairs"

COUNT=$(wc -l < "$WORK/pairs" | tr -d ' ')
if [ "$COUNT" -eq 0 ]; then
    fail "formula lists no url/sha256 pairs"
fi

PREFIX="https://github.com/${REPO}/releases/download/${TAG}/${CRATE_NAME}-"
while read -r url sha; do
    name=${url##*/}
    case "$url" in
        "$PREFIX"*.tar.xz) ;;
        *)
            fail "${name:-<empty url>}: url is not a ${REPO} ${TAG} release asset (${url})"
            continue
            ;;
    esac
    case "$sha" in
        *[!0-9a-f]* | '') fail "${name}: sha256 is not lowercase hex"; continue ;;
    esac
    if [ "${#sha}" -ne 64 ]; then
        fail "${name}: sha256 has the wrong length"
        continue
    fi

    asset="$WORK/$name"
    if ! curl -fsSL "$url" -o "$asset"; then
        fail "${name}: download failed"
        continue
    fi
    actual=$(sha256_of "$asset")
    if [ "$actual" != "$sha" ]; then
        fail "${name}: formula sha256 ${sha} != release asset ${actual}"
        continue
    fi
    if gh attestation verify "$asset" \
        --repo "$REPO" \
        --signer-workflow "${REPO}/.github/workflows/release.yml" \
        --source-ref "refs/tags/${TAG}" >/dev/null 2>&1; then
        pass "${name}: sha256 matches and provenance verified"
    else
        fail "${name}: no valid build provenance from ${REPO} release.yml at ${TAG}"
    fi
done < "$WORK/pairs"

echo ""
if [ "$FAIL" -eq 0 ]; then
    echo "Homebrew tap integrity intact (${COUNT} archives)"
else
    echo "Homebrew tap integrity REGRESSED — see failures above" >&2
    exit 1
fi
