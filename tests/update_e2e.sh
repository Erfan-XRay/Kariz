#!/usr/bin/env bash
# Updating the panel from the panel, for real, on a systemd host where the manager has
# installed it (CI's manager job): a local server plays GitHub's releases, signed with a key
# made for the test (pinned in panel.toml). First a release whose program does not come up as
# the version it claims: the swap must be rolled back and the old panel must be back. Then a
# good one: the panel must come back as the new version, with the old programs kept.
#
# usage: update_e2e.sh NEWER-kariz-panel KARIZ   (the first built with KARIZ_BUILD_VERSION=99.0.0)
set -euo pipefail

NEW=$(readlink -f "${1:?usage: update_e2e.sh NEWER-kariz-panel KARIZ}")
CORE=$(readlink -f "${2:?usage: update_e2e.sh NEWER-kariz-panel KARIZ}")
WORK=$(mktemp -d)
PORT=18999
arch=$(uname -m)
conf=/etc/kariz-panel/panel.toml
cleanup() {
    if [[ -n "${SERVER_PID:-}" ]]; then kill "$SERVER_PID" 2>/dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

# a key for the test, and the two files a release repository is: the listing and the assets
out=$("$NEW" release-key --out "$WORK/key")
hexkey=$(sed -n 's/^public key (hex): //p' <<<"$out")
mkdir -p "$WORK/www/repos/x/y" "$WORK/www/dl"

make_release() { # tag: a release of the newer program under that tag
    local tag=$1 name="kariz-$1-$arch-linux"
    rm -rf "$WORK/www/dl"/* "$WORK/pack"
    mkdir -p "$WORK/pack/$name"
    cp "$NEW" "$WORK/pack/$name/kariz-panel"
    cp "$CORE" "$WORK/pack/$name/kariz"
    tar -czf "$WORK/www/dl/$name.tar.gz" -C "$WORK/pack" "$name"
    (cd "$WORK/www/dl" && sha256sum "$name.tar.gz" >"$name.tar.gz.sha256")
    KARIZ_SIGNING_KEY=$(cat "$WORK/key") "$NEW" release-sign "$WORK/www/dl/$name.tar.gz.sha256"
    local assets=""
    for f in "$name.tar.gz" "$name.tar.gz.sha256" "$name.tar.gz.sig"; do
        assets+="{\"name\":\"$f\",\"browser_download_url\":\"http://127.0.0.1:$PORT/dl/$f\"},"
    done
    printf '[{"tag_name":"%s","body":"a test release","prerelease":false,"draft":false,"assets":[%s]}]' \
        "$tag" "${assets%,}" >"$WORK/www/repos/x/y/releases"
}

(cd "$WORK/www" && exec python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>&1) &
SERVER_PID=$!

# the panel looks for releases here, and trusts this key
{
    echo "release_api = \"http://127.0.0.1:$PORT/repos/x/y\""
    echo "release_key = \"$hexkey\""
} >>"$conf"
systemctl restart kariz-panel
sleep 2

link=$(kariz-manager panel link --host 127.0.0.1 | grep -o 'https://[^ ]*#t=[0-9a-f]*')
base=${link%%#*}
token=${link##*#t=}
curl -skf -c "$WORK/jar" -H 'Content-Type: application/json' -d "{\"token\":\"$token\"}" "${base}api/link" >"$WORK/signin.json"
csrf=$(sed -n 's/.*"csrf":"\([0-9a-f]*\)".*/\1/p' "$WORK/signin.json")
# While the panel restarts these get "connection refused": that is not a failure of the
# test, the loops below just ask again.
api() { curl -sk -b "$WORK/jar" -H "X-Kariz-CSRF: $csrf" -H 'Content-Type: application/json' "$@" || true; }
version() { { curl -sk "${base}api/version" || true; } | sed -n 's/.*"version":"\([^"]*\)".*/\1/p'; }
original=$(version)
echo "the panel is $original"

# nothing to find yet
make_release "v$original"
test "$(api -X POST "${base}api/update/check" -d '{}' | jq -r .state)" = same

# ---- a release whose program does not answer as the version it claims: rolled back ----
make_release v98.0.0
status=$(api -X POST "${base}api/update/check" -d '{}')
test "$(jq -r .state <<<"$status")" = newer
test "$(jq -r .latest.version <<<"$status")" = 98.0.0
# a new major version needs confirmation
test "$(api -o /dev/null -w '%{http_code}' -X POST "${base}api/update/apply" -d '{}')" = 409
api -X POST "${base}api/update/apply" -d '{"confirm_major":true}' | jq -e .op >/dev/null
for _ in $(seq 1 90); do
    result=$(api "${base}api/update" | jq -c .last_result 2>/dev/null || true)
    if [[ -n "$result" && "$result" != null ]]; then break; fi
    sleep 2
done
echo "result: $result"
test "$(jq -r .ok <<<"$result")" = false
test "$(jq -r .rolled_back <<<"$result")" = true
sleep 3
test "$(version)" = "$original"
systemctl is-active --quiet kariz-panel
test ! -e /usr/local/bin/kariz-panel.previous
echo "the failed update was rolled back and the old panel is running"

# ---- a good release ----
make_release v99.0.0
test "$(api -X POST "${base}api/update/check" -d '{}' | jq -r .latest.version)" = 99.0.0
api -X POST "${base}api/update/apply" -d '{"confirm_major":true}' | jq -e .op >/dev/null
for _ in $(seq 1 90); do
    if [[ "$(version)" == 99.0.0 ]]; then break; fi
    sleep 2
done
test "$(version)" = 99.0.0
for _ in $(seq 1 30); do
    result=$(api "${base}api/update" | jq -c .last_result 2>/dev/null || true)
    if [[ -n "$result" && "$(jq -r .version <<<"$result")" == 99.0.0 ]]; then break; fi
    sleep 1
done
echo "result: $result"
test "$(jq -r .ok <<<"$result")" = true
systemctl is-active --quiet kariz-panel
test -x /usr/local/bin/kariz-panel.previous
test -x /usr/local/bin/kariz.previous
# the session survived the restart (sessions are in the database)
test "$(api "${base}api/update" | jq -r .current)" = 99.0.0
echo "the update to 99.0.0 worked"

# ---- the other servers: the agent (this same machine, connected as ci-agent) is still the
# old program in memory; the panel sends it the release, it swaps, restarts and reconnects,
# then the tunnels are restarted one at a time ----
agent_version() { api "${base}api/servers" | jq -r '.servers[] | select(.local == false) | .version' | head -n1; }
test "$(api "${base}api/update" | jq -r '.outdated | length')" = 1
echo "the agent is $(agent_version), the panel 99.0.0"
before=$(systemctl show kariz@plain -p ActiveEnterTimestampMonotonic --value)
op=$(api -X POST "${base}api/update/servers" -d '{"restart_tunnels":true}' | jq -r .op)
test -n "$op"
for _ in $(seq 1 120); do
    state=$(api "${base}api/op?id=$op" | jq -r .state 2>/dev/null || true)
    if [[ -n "$state" && "$state" != running ]]; then break; fi
    sleep 2
done
api "${base}api/op?id=$op" | jq -c '{state, error, steps: [.steps[].id]}'
test "$state" = done
test "$(agent_version)" = 99.0.0
test "$(api "${base}api/update" | jq -r '.outdated | length')" = 0
after=$(systemctl show kariz@plain -p ActiveEnterTimestampMonotonic --value)
test "$before" != "$after"
systemctl is-active --quiet kariz-agent
systemctl is-active --quiet kariz@plain
echo "the agent was updated over the link and its tunnels were restarted one at a time"
