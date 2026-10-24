#!/usr/bin/env sh
# Tests for install.sh path traversal check (issue #1250, CWE-22).
#
# Verifies:
#   1. Safe archives (single binary, "./prefix", subdirs) are accepted.
#   2. Archives with absolute paths are rejected pre-extraction.
#   3. Archives with ".." components are rejected pre-extraction.
#   4. The check is still present in install.sh (regression guard).
#   5. The installer requires and verifies a SHA-256 sidecar.
#   6. Build provenance: verified with an authenticated gh (a failure aborts),
#      skipped with a warning without one, enforced by BDO_VERIFY_ATTESTATION.
#   7. Every target install.sh picks is one the release actually publishes.

set -eu

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
INSTALL_SH="$REPO_ROOT/install.sh"

if [ ! -f "$INSTALL_SH" ]; then
    echo "FAIL: install.sh not found at $INSTALL_SH"
    exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "SKIP: python3 not available — crafted tarball tests require python3"
    exit 0
fi

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

# The check replicated from install.sh (keep in sync with install.sh).
# Returns 0 when archive is safe, 1 when unsafe.
check_archive() {
    if tar -tzf "$1" | grep -qE '^/|(^|/)\.\.(/|$)'; then
        return 1
    fi
    return 0
}

# --- Build safe archive using standard tar ---
mkdir -p "$TMPDIR/safe_src"
printf '#!/bin/sh\necho bdo\n' > "$TMPDIR/safe_src/bdo"
(cd "$TMPDIR/safe_src" && tar -czf "$TMPDIR/safe.tgz" bdo)

# --- Build crafted malicious archives with python ---
python3 - "$TMPDIR" <<'PY'
import sys, tarfile, io

base = sys.argv[1]


def make(name, entry):
    with tarfile.open(f"{base}/{name}", "w:gz") as t:
        info = tarfile.TarInfo(name=entry)
        data = b"pwned"
        info.size = len(data)
        t.addfile(info, io.BytesIO(data))


make("traversal.tgz", "../etc/evil")
make("absolute.tgz", "/tmp/evil_abs")
make("middle.tgz", "bdo/../../../etc/evil")
make("end_dotdot.tgz", "bdo/..")
PY

FAIL=0
pass() { printf '  PASS: %s\n' "$1"; }
fail() { printf '  FAIL: %s\n' "$1"; FAIL=1; }

echo "==> Functional checks"

if check_archive "$TMPDIR/safe.tgz"; then
    pass "safe archive accepted"
else
    fail "safe archive rejected (false positive)"
fi

for bad in traversal absolute middle end_dotdot; do
    if check_archive "$TMPDIR/$bad.tgz"; then
        fail "$bad archive accepted (should be rejected)"
    else
        pass "$bad archive rejected"
    fi
done

echo "==> Regression guard"

if grep -qF 'tar -tf' "$INSTALL_SH" && grep -qF '\.\.' "$INSTALL_SH"; then
    pass "install.sh still contains the path-traversal check"
else
    fail "install.sh is missing the path-traversal check — was it removed?"
fi

if grep -qF '${DOWNLOAD_URL}.sha256' "$INSTALL_SH" \
    && grep -qF 'ACTUAL_SHA256' "$INSTALL_SH" \
    && grep -qF 'Checksum verification failed' "$INSTALL_SH"; then
    pass "install.sh verifies the release checksum before extraction"
else
    fail "install.sh is missing mandatory SHA-256 verification"
fi

# The archive is named after the crate and compressed with xz. Getting either
# wrong makes every download 404 — which is how this went unnoticed until the
# checksum work put the download path under scrutiny.
if grep -qF '${CRATE_NAME}-${TARGET}.tar.xz' "$INSTALL_SH"; then
    pass "install.sh builds the cargo-dist archive name"
else
    fail "install.sh archive name does not match the published release assets"
fi

# An explicit -z/-J is applied literally by GNU tar and fails on a valid
# archive compressed the other way; both tars auto-detect without one.
if grep -qE 'tar -[tx]zf|tar -[tx]Jf' "$INSTALL_SH"; then
    fail "install.sh forces a tar compression flag instead of auto-detecting"
else
    pass "install.sh lets tar auto-detect compression"
fi

# Verify the binary just installed, by path — resolving through PATH reports
# whatever other copy shadows it, which reads as a failed upgrade.
if grep -qF '"$INSTALLED_BIN" --version' "$INSTALL_SH" \
    && grep -qF 'not the copy just installed' "$INSTALL_SH"; then
    pass "install.sh verifies by path and warns on PATH shadowing"
else
    fail "install.sh verification can report a different binary than it installed"
fi

echo "==> Build provenance (attestation)"

# A fake gh records its argv and answers per FAKE_GH_AUTH / FAKE_GH_VERIFY.
mkdir -p "$TMPDIR/fakebin"
cat > "$TMPDIR/fakebin/gh" <<'GH'
#!/bin/sh
echo "$*" >> "$FAKE_GH_LOG"
case "$1 $2" in
    "auth status") exit "${FAKE_GH_AUTH:-0}" ;;
    "attestation verify") exit "${FAKE_GH_VERIFY:-0}" ;;
esac
exit 2
GH
chmod +x "$TMPDIR/fakebin/gh"
: > "$TMPDIR/archive"

# Run verify_attestation in a subshell (error() exits); echo the exit status.
attest() {
    (
        PATH="$1"
        FAKE_GH_LOG="$TMPDIR/gh.log" FAKE_GH_AUTH="$2" FAKE_GH_VERIFY="$3"
        VERSION="$4" BDO_VERIFY_ATTESTATION="$5"
        export PATH FAKE_GH_LOG FAKE_GH_AUTH FAKE_GH_VERIFY BDO_VERIFY_ATTESTATION
        BDO_INSTALL_SH_LIB=1 . "$INSTALL_SH"
        verify_attestation "$TMPDIR/archive"
    ) >/dev/null 2>&1 && echo 0 || echo 1
}
# PATHs hold only the fake (or nothing), so a real gh on the runner — GitHub's
# Ubuntu images ship one in /usr/bin — can never answer instead. Sourcing
# install.sh and verify_attestation need no external commands.
mkdir -p "$TMPDIR/emptybin"
WITH_GH="$TMPDIR/fakebin"
NO_GH="$TMPDIR/emptybin"
expect() { # description, expected, actual
    if [ "$2" = "$3" ]; then pass "$1"; else fail "$1 (exit $3, want $2)"; fi
}

: > "$TMPDIR/gh.log"
expect "verified provenance installs" 0 "$(attest "$WITH_GH" 0 0 v0.48.1 '')"
if grep -qF -- "attestation verify $TMPDIR/archive --repo tedorigawa001/TokenReductionTool-BDO --signer-workflow tedorigawa001/TokenReductionTool-BDO/.github/workflows/release.yml --source-ref refs/tags/v0.48.1" "$TMPDIR/gh.log"; then
    pass "gh pins repo, release workflow and tag"
else
    fail "gh attestation verify called without repo/workflow/tag pins: $(cat "$TMPDIR/gh.log")"
fi
expect "failed provenance refuses to install" 1 "$(attest "$WITH_GH" 0 1 v0.48.1 '')"
expect "BDO_VERIFY_ATTESTATION=0 skips a failing check" 0 "$(attest "$WITH_GH" 0 1 v0.48.1 0)"
expect "no gh: checksum only (auto)" 0 "$(attest "$NO_GH" 0 0 v0.48.1 '')"
expect "unauthenticated gh: checksum only (auto)" 0 "$(attest "$WITH_GH" 1 0 v0.48.1 '')"
expect "no gh + require refuses" 1 "$(attest "$NO_GH" 0 0 v0.48.1 require)"
expect "unauthenticated gh + require refuses" 1 "$(attest "$WITH_GH" 1 0 v0.48.1 require)"
: > "$TMPDIR/gh.log"
expect "pre-attestation release (v0.47.0): checksum only" 0 "$(attest "$WITH_GH" 0 1 v0.47.0 '')"
if [ -s "$TMPDIR/gh.log" ]; then
    fail "gh was called for a release that has no attestations"
else
    pass "no gh call for a pre-attestation release"
fi
expect "pre-attestation release + require refuses" 1 "$(attest "$WITH_GH" 0 0 v0.47.0 require)"
expect "v0.47.1 is the first attested release" 1 "$(attest "$WITH_GH" 0 1 v0.47.1 '')"
expect "v1.0.0 counts as attested" 1 "$(attest "$WITH_GH" 0 1 v1.0.0 '')"
expect "unknown BDO_VERIFY_ATTESTATION value is an error" 1 "$(attest "$WITH_GH" 0 0 v0.48.1 maybe)"

if grep -qE '^[[:space:]]*verify_attestation "\$ARCHIVE"' "$INSTALL_SH"; then
    pass "install() calls verify_attestation before extraction"
else
    fail "install() no longer calls verify_attestation"
fi

echo "==> Targets match published release assets"

# x86_64 Linux once asked for a musl archive the release never publishes.
DIST_TARGETS=$(sed -n 's/^targets = \[\(.*\)\]/\1/p' "$REPO_ROOT/dist-workspace.toml" | tr -d '" ' | tr ',' ' ')
for os_arch in "linux x86_64" "linux aarch64" "darwin x86_64" "darwin aarch64"; do
    # shellcheck disable=SC2086
    set -- $os_arch
    target=$(OS=$1 ARCH=$2 BDO_INSTALL_SH_LIB=1 sh -c '. "$0"; get_target; echo "$TARGET"' "$INSTALL_SH")
    case " $DIST_TARGETS " in
        *" $target "*) pass "$1/$2 -> $target is published" ;;
        *) fail "$1/$2 -> $target is not in dist-workspace.toml targets ($DIST_TARGETS)" ;;
    esac
done

echo ""
if [ "$FAIL" -eq 0 ]; then
    echo "All install.sh path traversal tests passed"
    exit 0
else
    echo "Some tests failed"
    exit 1
fi
