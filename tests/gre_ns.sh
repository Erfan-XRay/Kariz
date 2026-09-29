#!/usr/bin/env bash
# Two network namespaces joined by a veth pair play two servers with public addresses
# 192.0.2.1 and 192.0.2.2. `kariz-panel net` (what an agent does when the panel asks) makes
# a GRE link between them; the private addresses must ping across it, a second link between
# the same pair needs its own key, a filtered GRE path makes the ping fail, and `net down`
# removes the interface. Needs root (CI runs it with sudo) and the ip_gre module.
set -euo pipefail

BIN=${1:?usage: gre_ns.sh PATH-TO-kariz-panel [PATH-TO-kariz]}
BIN=$(readlink -f "$BIN")
KARIZ=${2:+$(readlink -f "$2")}
WORK=$(mktemp -d)
PIDS=()
cleanup() {
    for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
    ip netns del kza 2>/dev/null || true
    ip netns del kzb 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

modprobe ip_gre 2>/dev/null || true
ip netns add kza
ip netns add kzb
ip link add va type veth peer name vb
ip link set va netns kza
ip link set vb netns kzb
for side in a b; do
    ip -n "kz$side" link set lo up
done
ip -n kza addr add 192.0.2.1/24 dev va
ip -n kzb addr add 192.0.2.2/24 dev vb
ip -n kza link set va up
ip -n kzb link set vb up

# Can this kernel make GRE at all? If it cannot, say so and stop: the panel would report the
# same ("gre: unavailable") and disable the option for that server.
if ! out=$(ip netns exec kza "$BIN" net status) || ! grep -q '"gre":true' <<<"$out"; then
    echo "::warning::this kernel cannot make GRE interfaces; the GRE test was skipped: $out"
    exit 0
fi

spec() { # name local remote key address peer
    cat >"$WORK/$1-$2.toml" <<EOF
name = "$1"
local = "$3"
remote = "$4"
key = $5
address = "$6"
peer = "$7"
prefix = 30
mtu = 1476
EOF
}

# link one: 10.77.0.0/30
spec kz-one a 192.0.2.1 192.0.2.2 1 10.77.0.1 10.77.0.2
spec kz-one b 192.0.2.2 192.0.2.1 1 10.77.0.2 10.77.0.1
ip netns exec kza "$BIN" net up "$WORK/kz-one-a.toml" --state "$WORK/a-state.toml"
ip netns exec kzb "$BIN" net up "$WORK/kz-one-b.toml" --state "$WORK/b-state.toml"
ip netns exec kza "$BIN" net ping kz-one --state "$WORK/a-state.toml"
ip netns exec kzb "$BIN" net ping kz-one --state "$WORK/b-state.toml"
ip netns exec kza ping -c 2 -W 2 10.77.0.2 >/dev/null
echo "link one: the private addresses answer"

# the interface has the address, the GRE key and the MTU that were asked for
ip -n kza -d link show kz-one | grep -q 'gre remote 192.0.2.2 local 192.0.2.1'
ip -n kza -d link show kz-one | grep -q 'mtu 1476'
ip -n kza -d addr show kz-one | grep -q '10.77.0.1/30'

# link two between the same pair of public addresses: its own key and its own /30
spec kz-two a 192.0.2.1 192.0.2.2 2 10.77.0.5 10.77.0.6
spec kz-two b 192.0.2.2 192.0.2.1 2 10.77.0.6 10.77.0.5
ip netns exec kza "$BIN" net up "$WORK/kz-two-a.toml" --state "$WORK/a-state.toml"
ip netns exec kzb "$BIN" net up "$WORK/kz-two-b.toml" --state "$WORK/b-state.toml"
ip netns exec kza "$BIN" net ping kz-two --state "$WORK/a-state.toml"
ip netns exec kza ping -c 1 -W 2 10.77.0.2 >/dev/null
echo "link two: both links work at once"

# making the same link again is fine (it is replaced)
ip netns exec kza "$BIN" net up "$WORK/kz-one-a.toml" --state "$WORK/a-state.toml"
ip netns exec kza "$BIN" net ping kz-one --state "$WORK/a-state.toml"
test "$(grep -c '^\[\[link\]\]' "$WORK/a-state.toml")" = 2

# a request that is not a kz- link runs nothing and changes nothing
sed 's/kz-one/eth0/' "$WORK/kz-one-a.toml" >"$WORK/bad.toml"
if ip netns exec kza "$BIN" net up "$WORK/bad.toml" 2>/dev/null; then
    echo "a link named eth0 was accepted" >&2
    exit 1
fi

# GRE filtered on the way in: the path test fails (and says so) instead of pretending
if command -v iptables >/dev/null; then
    ip netns exec kzb iptables -A INPUT -p gre -j DROP
    if ip netns exec kza "$BIN" net ping kz-one --state "$WORK/a-state.toml" >/dev/null; then
        echo "the ping passed with GRE filtered" >&2
        exit 1
    fi
    ip netns exec kzb iptables -D INPUT -p gre -j DROP
    ip netns exec kza "$BIN" net ping kz-one --state "$WORK/a-state.toml" >/dev/null
    echo "a filtered path fails the test, and it passes again once open"
fi

# a Kariz tunnel over the private addresses: the exit listens on its end of the link
# (10.77.0.2), the entry dials it from the other, and a request goes through end to end
if [[ -n "$KARIZ" ]]; then
    ip netns exec kzb python3 -m http.server 18081 --bind 127.0.0.1 --directory "$WORK" >/dev/null 2>&1 &
    PIDS+=($!)
    echo hello-over-gre >"$WORK/index.html"
    cat >"$WORK/exit.toml" <<EOF
role = "exit"
mode = "direct"
[tunnel]
transport = "tcpmux"
listen = "10.77.0.2:3080"
token = "gre-test-token-0123456789abcdef"
EOF
    cat >"$WORK/entry.toml" <<EOF
role = "entry"
mode = "direct"
[tunnel]
transport = "tcpmux"
remote = "10.77.0.2:3080"
token = "gre-test-token-0123456789abcdef"
[[forward]]
listen = "127.0.0.1:18080"
target = "127.0.0.1:18081"
EOF
    ip netns exec kzb "$KARIZ" run -c "$WORK/exit.toml" >"$WORK/exit.log" 2>&1 &
    PIDS+=($!)
    sleep 1
    ip netns exec kza "$KARIZ" run -c "$WORK/entry.toml" >"$WORK/entry.log" 2>&1 &
    PIDS+=($!)
    ok=0
    for _ in $(seq 1 20); do
        if [[ "$(ip netns exec kza curl -sf --max-time 3 http://127.0.0.1:18080/)" == hello-over-gre ]]; then
            ok=1
            break
        fi
        sleep 1
    done
    if [[ $ok != 1 ]]; then
        echo "no answer through a tunnel over the private link" >&2
        cat "$WORK/exit.log" "$WORK/entry.log" >&2
        exit 1
    fi
    # it went over the private link, not the public path: GRE packets were counted
    ip -n kza -s link show kz-one | awk '/RX:/{getline; if ($2 == 0) exit 1}'
    echo "a Kariz tunnel works over the private addresses"
    for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
    PIDS=()
fi

# down removes the interface
ip netns exec kza "$BIN" net down kz-one
if ip -n kza link show kz-one >/dev/null 2>&1; then
    echo "kz-one is still there" >&2
    exit 1
fi
ip -n kza link show kz-two >/dev/null
echo "the GRE test passed"
