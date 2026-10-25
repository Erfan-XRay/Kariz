#!/usr/bin/env bash
# `kariz-manager panel port` and a certificate of your own (issue #95): which ports the panel can
# take, the port written into its settings (and put back when the panel does not start), and a
# certificate and key checked before the panel is pointed at them. systemctl and what listens
# on a port are stand-ins; the certificates are made with openssl.
# Usage: bash tests/panel_port.sh (no root, nothing on the system is touched).
set -euo pipefail

# shellcheck source=scripts/kariz.sh
source "$(dirname "$0")/../scripts/kariz.sh"
set +e

dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
PANEL_CONF=$dir/panel.toml
PANEL_DOMAIN=$dir/domain
cat >"$PANEL_CONF" <<'EOF'
listen = "[::]:28443"
path = "k-test12"
agent_listen = "0.0.0.0:29001"
EOF

# What listens where: "port unit comm" per line.
LISTENING="28443 kariz-panel.service kariz-panel
8443 nginx.service nginx"
port_users() {
    awk -v p="$1" '$1 == p { print $2, $3, 1 }' <<<"$LISTENING"
}
# The panel starts, unless STARTS is 0; what systemctl was asked goes to a file.
STARTS=1
systemctl() {
    echo "$*" >>"$dir/systemctl"
    case $1 in
        is-active) ((STARTS)) ;;
    esac
}
need_root() { :; }
need_systemd() { :; }
sleep() { :; }
panel_show() { :; }
# The real one refuses /tmp, where these files are.
own_file() { realpath "$1"; }

fail() {
    echo "FAILED: $*" >&2
    exit 1
}
problem() { panel_port_problem "$1"; }

# Which ports the panel can take.
for p in abc 0 65536 -1 ""; do
    [[ "$(problem "$p")" == *"number from 1 to 65535"* ]] || fail "port '$p' was taken"
done
[[ "$(problem 80)" == *"Let's Encrypt"* ]] || fail "port 80 was taken"
[[ "$(problem 29001)" == *"29001 and 29002"* ]] || fail "the agents' port was taken"
[[ "$(problem 29002)" == *"29001 and 29002"* ]] || fail "the agents' second port was taken"
[[ "$(problem 8443)" == "nginx listens on it already" ]] || fail "a port nginx has was taken: $(problem 8443)"
[[ -z "$(problem 28443)" ]] || fail "the panel's own port was refused: $(problem 28443)"
[[ -z "$(problem 443)" ]] || fail "a free port was refused: $(problem 443)"
[[ -z "$(problem 0443)" ]] || fail "a port with a leading zero was refused"

# A new port: written with the address kept, and the panel restarted.
(panel_port 443) >/dev/null 2>&1 || fail "panel port 443 failed"
[[ "$(panel_setting listen)" == "[::]:443" ]] || fail "listen is $(panel_setting listen)"
[[ "$(panel_setting agent_listen)" == "0.0.0.0:29001" ]] || fail "the agents' port changed"
grep -q "restart kariz-panel" "$dir/systemctl" || fail "the panel was not restarted"

# The same port again: nothing is restarted.
: >"$dir/systemctl"
(panel_port 443) >/dev/null 2>&1 || fail "panel port with its own port failed"
[[ ! -s "$dir/systemctl" ]] || fail "the panel was restarted for its own port"

# A port that is refused changes nothing.
(panel_port 8443) >/dev/null 2>&1 && fail "panel port took nginx's port"
[[ "$(panel_setting listen)" == "[::]:443" ]] || fail "a refused port was written"

# The panel does not start on the new port: it goes back to the one it had.
STARTS=0
(panel_port 2053) >/dev/null 2>&1 && fail "a panel that did not start was not reported"
[[ "$(panel_setting listen)" == "[::]:443" ]] || fail "listen is $(panel_setting listen) after a failed start"
STARTS=1

# Asked: a refused answer is asked again, Enter keeps the port it has.
printf '80\n2053\n' >"$dir/answers"
IN_FD="" KARIZ_INPUT=$dir/answers
answer=""
ask_panel_port answer 443 >/dev/null 2>&1
[[ "$answer" == 2053 ]] || fail "asked port: $answer"
printf '\n' >"$dir/answers"
IN_FD=""
ask_panel_port answer 443 >/dev/null 2>&1
[[ "$answer" == 443 ]] || fail "Enter did not keep the port: $answer"
printf '\n' >"$dir/answers"
IN_FD=""
ask_panel_port answer "" >/dev/null 2>&1
[[ -z "$answer" ]] || fail "Enter at install did not leave a random port: $answer"
IN_FD="" KARIZ_INPUT=""

# A certificate of your own: the key must be the certificate's.
mkcert() {
    openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days "$3" \
        -subj /CN=panel.example.com -keyout "$2" -out "$1" >/dev/null 2>&1
}
mkcert "$dir/a.pem" "$dir/a.key" 30
mkcert "$dir/b.pem" "$dir/b.key" 30
(check_own_cert "$dir/a.pem" "$dir/a.key") >/dev/null 2>&1 || fail "a good certificate was refused"
(check_own_cert "$dir/a.pem" "$dir/b.key") >/dev/null 2>&1 && fail "another certificate's key was taken"
(check_own_cert "$dir/a.key" "$dir/a.key") >/dev/null 2>&1 && fail "a key was taken as the certificate"
(check_own_cert "$dir/a.pem" "$dir/none.key") >/dev/null 2>&1 && fail "a missing key was taken"

# It is used, and the Let's Encrypt one it had is forgotten.
echo panel.example.com >"$PANEL_DOMAIN"
(use_own_cert "$dir/a.pem" "$dir/a.key") >/dev/null 2>&1 || fail "use_own_cert failed"
[[ "$(panel_setting cert_file)" == "$(realpath "$dir/a.pem")" ]] || fail "cert_file is $(panel_setting cert_file)"
[[ ! -e "$PANEL_DOMAIN" ]] || fail "the Let's Encrypt domain was kept"
first=""
cert_situation first >/dev/null 2>&1
[[ "$first" == own ]] || fail "the certificate screen offers $first first, not own"

# The panel does not start with the new one: the one it had stays.
STARTS=0
(use_own_cert "$dir/b.pem" "$dir/b.key") >/dev/null 2>&1 && fail "a panel that did not start was not reported"
[[ "$(panel_setting cert_file)" == "$(realpath "$dir/a.pem")" ]] || fail "the old certificate was not put back"
[[ "$(panel_setting key_file)" == "$(realpath "$dir/a.key")" ]] || fail "the old key was not put back"
STARTS=1

echo "panel port and own certificate: ok"
