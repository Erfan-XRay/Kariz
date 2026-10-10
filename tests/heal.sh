#!/usr/bin/env bash
# `kariz-manager panel heal`: when the server's IP address changes and the panel's certificate
# was for the old one, a certificate for the new address is asked for, once an hour at most
# after a failure, and never behind NAT or for a domain. The addresses come from a stand-in for
# `ip`, and getting the certificate is a stand-in that writes down what it was asked.
# Usage: bash tests/heal.sh (no root, nothing on the system is touched).
set -euo pipefail

# shellcheck source=scripts/kariz.sh
source "$(dirname "$0")/../scripts/kariz.sh"
set +e

dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
PANEL_CONF=$dir/panel.toml
PANEL_DOMAIN=$dir/domain
PANEL_DATA=$dir/data
HEAL_FAILED=$PANEL_DATA/heal-failed
printf 'listen = "0.0.0.0:28443"\npath = "k-test12"\n' >"$PANEL_CONF"

# What this server has: its addresses and the source address of its route out.
ADDRS="" ROUTE=""
ip() {
    case "$*" in
        "-4 -o addr show")
            local a n=1
            for a in $ADDRS; do
                printf '%s: eth0    inet %s/24 brd x scope global eth0\n' "$n" "$a"
                n=$((n + 1))
            done
            ;;
        "-4 route get 1.1.1.1") [[ -z "$ROUTE" ]] || printf '1.1.1.1 via x dev eth0 src %s uid 0\n' "$ROUTE" ;;
    esac
}
need_root() { :; }
# panel_heal runs it in a subshell: what it was asked goes to a file.
: >"$dir/asked"
WORKS=1
switch_cert() {
    printf ' %s/%s' "$1" "$2" >>"$dir/asked"
    ((WORKS)) || return 1
    printf '%s\n' "$1" >"$PANEL_DOMAIN"
}

fail() {
    echo "FAILED: $*" >&2
    exit 1
}
heal() {
    local r=0
    panel_heal >/dev/null 2>&1 || r=$?
    ASKED=$(cat "$dir/asked")
    return $r
}

# The address is still here: nothing.
echo 203.0.113.5 >"$PANEL_DOMAIN"
ADDRS="203.0.113.5" ROUTE=203.0.113.5
panel_new_ip >/dev/null && fail "the address is here, but a new one was found"
heal
[[ -z "$ASKED" ]] || fail "asked for a certificate with the address here:$ASKED"

# A second address on the server, the certificate's still among them: nothing.
ADDRS="198.51.100.7 203.0.113.5" ROUTE=198.51.100.7
heal
[[ -z "$ASKED" ]] || fail "asked for a certificate with the old address still here:$ASKED"

# Behind NAT: the certificate's address was typed and is on no interface; the route says only
# a private address. Nothing can be known from here.
ADDRS="10.0.0.4" ROUTE=10.0.0.4
heal
[[ -z "$ASKED" ]] || fail "asked for a certificate behind NAT:$ASKED"

# A domain: nothing, whatever the addresses.
echo panel.example.com >"$PANEL_DOMAIN"
ADDRS="198.51.100.7" ROUTE=198.51.100.7
heal
[[ -z "$ASKED" ]] || fail "asked for a certificate for a domain:$ASKED"

# A certificate of your own (no domain file): nothing.
rm -f "$PANEL_DOMAIN"
heal
[[ -z "$ASKED" ]] || fail "asked for a certificate with one of your own:$ASKED"

# The address changed: a certificate for the new one, and the panel's address follows.
echo 203.0.113.5 >"$PANEL_DOMAIN"
[[ "$(panel_new_ip)" == 198.51.100.7 ]] || fail "the new address was not found"
[[ "$(panel_host "")" == 198.51.100.7 ]] || fail "the panel's address is not the new one"
# The menu's certificate screen says so, and offers the IP address first.
said=$(cert_situation first 2>&1)
[[ "$said" == *"is 198.51.100.7 now"* ]] || fail "the certificate screen did not say the address changed: $said"
first=""
cert_situation first >/dev/null 2>&1
[[ "$first" == ip ]] || fail "the certificate screen offers $first first"
heal || fail "heal failed"
[[ "$ASKED" == " 198.51.100.7/ip" ]] || fail "asked for:$ASKED"
[[ "$(panel_identity)" == 198.51.100.7 ]] || fail "the certificate is not for the new address"
heal
[[ "$ASKED" == " 198.51.100.7/ip" ]] || fail "asked again after it worked:$ASKED"

# It fails (port 80 closed): written down, not tried again for an hour, then tried again.
WORKS=0
: >"$dir/asked"
ADDRS="192.0.2.9" ROUTE=192.0.2.9
heal && fail "a failure was not reported"
heal
[[ "$ASKED" == " 192.0.2.9/ip" ]] || fail "tried again within the hour:$ASKED"
read -r a t <"$HEAL_FAILED"
[[ "$a" == 192.0.2.9 ]] || fail "the failure was written down for $a"
printf '%s %s\n' "$a" $((t - 3601)) >"$HEAL_FAILED"
heal
[[ "$ASKED" == " 192.0.2.9/ip 192.0.2.9/ip" ]] || fail "not tried again after the hour:$ASKED"

# Another new address after a failure is tried at once; success clears the failure.
WORKS=1
ADDRS="192.0.2.10" ROUTE=192.0.2.10
heal || fail "heal failed"
[[ "$ASKED" == " 192.0.2.9/ip 192.0.2.9/ip 192.0.2.10/ip" ]] || fail "a new address waited for the old one's hour:$ASKED"
[[ ! -e "$HEAL_FAILED" ]] || fail "the failure was kept after it worked"

echo "panel heal: ok"
