#!/usr/bin/env bash
# Installing from a release archive on the server, without GitHub (docs/offline.md): the
# archive next to an unpacked copy of the script is found by itself, checked against its
# signature (a key made for the test), unpacked, and kept for the panel to update agents
# with. A damaged or unsigned archive is refused. Needs bash, openssl, tar and sha256sum;
# no root: the programs go to a temporary directory.
# The variables set after `source` are read by the sourced functions.
# shellcheck disable=SC2030,SC2031,SC2034
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
fail() {
    echo "FAIL: $*" >&2
    exit 1
}

case "$(uname -m)" in
    x86_64 | amd64) A=x86_64 ;;
    aarch64 | arm64) A=aarch64 ;;
    *) A=armv7 ;;
esac
V=v9.9.9
NAME="kariz-$V-$A-linux"

# A release, as the release workflow packs it, signed with a key made here.
openssl genpkey -algorithm ed25519 -out "$WORK/key.pem" 2>/dev/null
PEM=$(openssl pkey -in "$WORK/key.pem" -pubout)
mkdir -p "$WORK/pack/$NAME/scripts" "$WORK/srv"
printf '#!/bin/sh\necho "kariz 9.9.9"\n' >"$WORK/pack/$NAME/kariz"
printf '#!/bin/sh\necho "kariz-panel 9.9.9"\n' >"$WORK/pack/$NAME/kariz-panel"
chmod +x "$WORK/pack/$NAME/kariz" "$WORK/pack/$NAME/kariz-panel"
cp "$ROOT/scripts/kariz.sh" "$WORK/pack/$NAME/scripts/"
tar -czf "$WORK/srv/$NAME.tar.gz" -C "$WORK/pack" "$NAME"
(cd "$WORK/srv" && sha256sum "$NAME.tar.gz" >"$NAME.tar.gz.sha256")
openssl pkeyutl -sign -inkey "$WORK/key.pem" -rawin -in "$WORK/srv/$NAME.tar.gz.sha256" \
    -out "$WORK/srv/$NAME.tar.gz.sig"
# An older archive beside it: the newest is the one taken.
cp "$WORK/srv/$NAME.tar.gz" "$WORK/srv/kariz-v9.9.1-$A-linux.tar.gz"
# On the server: the files copied over and unpacked, as the guide says.
tar -xzf "$WORK/srv/$NAME.tar.gz" -C "$WORK/srv"

# Runs a function of the unpacked script, with the programs going to $WORK/root.
run() {
    (
        cd "$WORK"
        export KARIZ_RELEASE_KEY_PEM=$PEM
        # shellcheck disable=SC1090
        source "$WORK/srv/$NAME/scripts/kariz.sh"
        BIN=$WORK/root/kariz PANEL_BIN=$WORK/root/kariz-panel PANEL_DATA=$WORK/root/data
        MANAGER=$WORK/root/kariz-manager
        "$@"
    ) </dev/null
}
mkdir -p "$WORK/root"

[[ "$(run archive_version "/x/$NAME.tar.gz")" == "$V" ]] || fail "archive_version"
run archive_version "/x/kariz-$V-other-linux.tar.gz" >/dev/null && fail "another CPU's archive was taken"
found=$(run find_local_archive)
[[ "$found" == "$WORK/srv/$NAME.tar.gz" ]] || fail "find_local_archive gave '$found'"

run install_from_archive "$found" ""
[[ "$("$WORK/root/kariz")" == "kariz 9.9.9" ]] || fail "kariz not installed"
[[ "$("$WORK/root/kariz-panel")" == "kariz-panel 9.9.9" ]] || fail "kariz-panel not installed"
for e in tar.gz tar.gz.sha256 tar.gz.sig; do
    [[ -s "$WORK/root/data/updates/9.9.9/release/$NAME.$e" ]] || fail "the panel's copy of .$e is missing"
done
run install_from_archive "$found" v9.9.8 2>/dev/null && fail "a version that does not match passed"

# A damaged archive, a signature of another key, and no proofs at all are refused.
mkdir "$WORK/bad"
cp "$WORK/srv/$NAME.tar.gz"* "$WORK/bad/"
echo x >>"$WORK/bad/$NAME.tar.gz"
run install_from_archive "$WORK/bad/$NAME.tar.gz" "" 2>/dev/null && fail "a damaged archive passed"
cp "$WORK/srv/$NAME.tar.gz" "$WORK/bad/$NAME.tar.gz"
(
    export KARIZ_RELEASE_KEY_PEM=""
    # shellcheck disable=SC1090
    source "$WORK/srv/$NAME/scripts/kariz.sh"
    BIN=$WORK/root/kariz PANEL_BIN=$WORK/root/kariz-panel PANEL_DATA=$WORK/root/data
    install_from_archive "$WORK/bad/$NAME.tar.gz" ""
) </dev/null 2>/dev/null && fail "a signature of another key passed"
rm "$WORK/bad/$NAME.tar.gz.sha256" "$WORK/bad/$NAME.tar.gz.sig"
run install_from_archive "$WORK/bad/$NAME.tar.gz" "" 2>/dev/null && fail "an archive without proofs passed unasked"

echo "offline install: ok"
