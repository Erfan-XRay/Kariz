#!/usr/bin/env bash
# Two network namespaces joined by a veth pair play two servers with public addresses
# 192.0.2.1 and 192.0.2.2. `kariz-panel net` (what an agent does when the panel asks) makes
# a GRE link between them; the private addresses must ping across it, a second link between
# the same pair needs its own key, a filtered GRE path makes the ping fail, and `net down`
# removes the interface. Then the same for VXLAN links (UDP, a VNI each, a port per network),
# and for links between IPv6 addresses (ip6gre, VXLAN over IPv6).
# Needs root (CI runs it with sudo) and the ip_gre module; VXLAN is skipped without its module.
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
modprobe vxlan 2>/dev/null || true
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
mtu = ${MTU:-1472}
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
ip -n kza -d link show kz-one | grep -q 'mtu 1472'
ip -n kza -d addr show kz-one | grep -q '10.77.0.1/30'

# a new MTU is set in place: the interface stays the same one (its index does not change)
index() { ip -n kza -o link show kz-one | cut -d: -f1; }
before=$(index)
MTU=1372 spec kz-one a 192.0.2.1 192.0.2.2 1 10.77.0.1 10.77.0.2
ip netns exec kza "$BIN" net up "$WORK/kz-one-a.toml" --state "$WORK/a-state.toml"
ip -n kza -d link show kz-one | grep -q 'mtu 1372'
test "$(index)" = "$before"
spec kz-one a 192.0.2.1 192.0.2.2 1 10.77.0.1 10.77.0.2
ip netns exec kza "$BIN" net up "$WORK/kz-one-a.toml" --state "$WORK/a-state.toml"
test "$(index)" = "$before"
echo "link one: its MTU changes in place"

# the path MTU between the servers is measured, and a smaller path is found
test "$(ip netns exec kza "$BIN" net pmtu 192.0.2.2)" = 1500
ip -n kza link set va mtu 1400
ip -n kzb link set vb mtu 1400
test "$(ip netns exec kza "$BIN" net pmtu 192.0.2.2)" = 1400
ip -n kza link set va mtu 1500
ip -n kzb link set vb mtu 1500
echo "the path MTU is measured: 1500, and 1400 on a smaller path"

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

# ---- VXLAN ----
vspec() { # name side local remote vni port address peer
    cat >"$WORK/$1-$2.toml" <<EOF
name = "$1"
local = "$3"
remote = "$4"
key = $5
address = "$7"
peer = "$8"
prefix = 30
mtu = 1450
kind = "vxlan"
port = $6
EOF
}
vup() { # name vni port address-a address-b
    vspec "$1" a 192.0.2.1 192.0.2.2 "$2" "$3" "$4" "$5"
    vspec "$1" b 192.0.2.2 192.0.2.1 "$2" "$3" "$5" "$4"
    ip netns exec kza "$BIN" net up "$WORK/$1-a.toml" --state "$WORK/a-state.toml"
    ip netns exec kzb "$BIN" net up "$WORK/$1-b.toml" --state "$WORK/b-state.toml"
}
if ! ip netns exec kza "$BIN" net status | grep -q '"vxlan":true'; then
    echo "::warning::this kernel cannot make VXLAN interfaces; the VXLAN test was skipped"
else
    vup kz-vx1 1 4789 10.78.0.1 10.78.0.2
    ip netns exec kza "$BIN" net ping kz-vx1 --state "$WORK/a-state.toml"
    ip netns exec kza ping -c 2 -W 2 10.78.0.2 >/dev/null
    # iproute2's wording differs between versions (Ubuntu adds fan-map fields): each part alone
    vx=$(ip -n kza -d link show kz-vx1)
    for want in 'vxlan id 1 ' 'remote 192.0.2.2' 'local 192.0.2.1' 'dstport 4789' 'mtu 1450'; do
        if ! grep -qF "$want" <<<"$vx"; then
            echo "kz-vx1 lacks '$want':" >&2
            echo "$vx" >&2
            exit 1
        fi
    done
    echo "vxlan one: the private addresses answer, over UDP 4789"

    # a second VXLAN link between the same servers needs its own VNI on that port...
    vspec kz-dup a 192.0.2.1 192.0.2.2 1 4789 10.78.0.9 10.78.0.10
    if ip netns exec kza "$BIN" net up "$WORK/kz-dup-a.toml" --state "$WORK/dup.toml" 2>/dev/null; then
        echo "the kernel took a VNI twice on one port; the panel's rule would not be needed" >&2
        exit 1
    fi
    vup kz-vx2 2 4789 10.78.0.5 10.78.0.6
    # ...while another network's port may use VNI 1 again
    vup kz-vx3 1 8472 10.79.0.1 10.79.0.2
    ip -n kza -d link show kz-vx3 | grep -q 'dstport 8472'
    for l in kz-vx1 kz-vx2 kz-vx3; do
        ip netns exec kza "$BIN" net ping "$l" --state "$WORK/a-state.toml" >/dev/null
    done
    ip netns exec kza ping -c 1 -W 2 10.78.0.2 >/dev/null
    echo "vxlan: three links at once, a VNI each per port; GRE still answers too"
    ip netns exec kza ping -c 1 -W 2 10.77.0.2 >/dev/null

    # its UDP port filtered on the way in: the path test fails, the other port still passes
    if command -v iptables >/dev/null; then
        ip netns exec kzb iptables -A INPUT -p udp --dport 4789 -j DROP
        if ip netns exec kza "$BIN" net ping kz-vx1 --state "$WORK/a-state.toml" >/dev/null; then
            echo "the ping passed with UDP 4789 filtered" >&2
            exit 1
        fi
        ip netns exec kza "$BIN" net ping kz-vx3 --state "$WORK/a-state.toml" >/dev/null
        ip netns exec kzb iptables -D INPUT -p udp --dport 4789 -j DROP
        ip netns exec kza "$BIN" net ping kz-vx1 --state "$WORK/a-state.toml" >/dev/null
        echo "vxlan: a filtered port fails the test, another port passes"
    fi

    # the saved list says VXLAN (the agent makes them again from it after a reboot)
    grep -q 'kind = "vxlan"' "$WORK/a-state.toml"
    grep -q 'port = 8472' "$WORK/a-state.toml"
    for l in kz-vx1 kz-vx2 kz-vx3; do
        ip netns exec kza "$BIN" net down "$l"
        if ip -n kza link show "$l" >/dev/null 2>&1; then
            echo "$l is still there" >&2
            exit 1
        fi
    done
    echo "the VXLAN test passed"
fi

# ---- over IPv6 ----
# The same servers with IPv6 addresses: an ip6gre link (and a VXLAN one) between them keeps
# an IPv4 /30 of its own. A full-size packet that may not be fragmented crosses it, which
# also shows the link has no 8-byte encapsulation limit option eating into its MTU.
ip -n kza addr add 2001:db8::1/64 dev va nodad
ip -n kzb addr add 2001:db8::2/64 dev vb nodad
s6() { # name side local remote key address peer mtu [kind port]
    cat >"$WORK/$1-$2.toml" <<EOF
name = "$1"
local = "$3"
remote = "$4"
key = $5
address = "$6"
peer = "$7"
prefix = 30
mtu = $8
EOF
    if [[ -n "${9:-}" ]]; then
        printf 'kind = "%s"\nport = %s\n' "$9" "${10}" >>"$WORK/$1-$2.toml"
    fi
}
if ! ip netns exec kza "$BIN" net status | grep -q '"gre6":true'; then
    echo "::warning::this kernel cannot make ip6gre interfaces; the IPv6 test was skipped"
else
    s6 kz-six a 2001:db8::1 2001:db8::2 1 10.80.0.1 10.80.0.2 1452
    s6 kz-six b 2001:db8::2 2001:db8::1 1 10.80.0.2 10.80.0.1 1452
    ip netns exec kza "$BIN" net up "$WORK/kz-six-a.toml" --state "$WORK/a-state.toml"
    ip netns exec kzb "$BIN" net up "$WORK/kz-six-b.toml" --state "$WORK/b-state.toml"
    ip netns exec kza "$BIN" net ping kz-six --state "$WORK/a-state.toml"
    ip -n kza -d link show kz-six | grep -q 'ip6gre'
    ip -n kza -d link show kz-six | grep -q 'mtu 1452'
    # 1452 bytes: 1424 of ICMP payload, 28 of IPv4 and ICMP headers
    ip netns exec kza ping -c 2 -W 2 -M 'do' -s 1424 10.80.0.2 >/dev/null
    echo "ip6gre: the private addresses answer, full-size packets cross whole"
    test "$(ip netns exec kza "$BIN" net pmtu 2001:db8::2)" = 1500
    echo "the path MTU is measured over IPv6"

    if command -v ip6tables >/dev/null; then
        ip netns exec kzb ip6tables -A INPUT -p gre -j DROP
        if ip netns exec kza "$BIN" net ping kz-six --state "$WORK/a-state.toml" >/dev/null; then
            echo "the ping passed with GRE over IPv6 filtered" >&2
            exit 1
        fi
        ip netns exec kzb ip6tables -D INPUT -p gre -j DROP
        ip netns exec kza "$BIN" net ping kz-six --state "$WORK/a-state.toml" >/dev/null
        echo "ip6gre: a filtered path fails the test"
    fi

    if ip netns exec kza "$BIN" net status | grep -q '"vxlan6":true'; then
        s6 kz-vsix a 2001:db8::1 2001:db8::2 9 10.80.0.5 10.80.0.6 1430 vxlan 4789
        s6 kz-vsix b 2001:db8::2 2001:db8::1 9 10.80.0.6 10.80.0.5 1430 vxlan 4789
        ip netns exec kza "$BIN" net up "$WORK/kz-vsix-a.toml" --state "$WORK/a-state.toml"
        ip netns exec kzb "$BIN" net up "$WORK/kz-vsix-b.toml" --state "$WORK/b-state.toml"
        ip netns exec kza "$BIN" net ping kz-vsix --state "$WORK/a-state.toml"
        ip netns exec kza ping -c 2 -W 2 -M 'do' -s 1402 10.80.0.6 >/dev/null
        echo "vxlan over IPv6: the private addresses answer, full-size packets cross whole"
        ip netns exec kza "$BIN" net down kz-vsix
    fi
    ip netns exec kza "$BIN" net down kz-six
    if ip -n kza link show kz-six >/dev/null 2>&1; then
        echo "kz-six is still there" >&2
        exit 1
    fi
    echo "the IPv6 test passed"
fi

# down removes the interface
ip netns exec kza "$BIN" net down kz-one
if ip -n kza link show kz-one >/dev/null 2>&1; then
    echo "kz-one is still there" >&2
    exit 1
fi
ip -n kza link show kz-two >/dev/null
echo "the GRE test passed"
