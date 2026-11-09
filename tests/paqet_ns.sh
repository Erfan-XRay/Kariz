#!/usr/bin/env bash
# Two Linux hosts on a veth pair exercise the reverse Paqet tunnel and its firewall.
set -euo pipefail

BIN=$(readlink -f "${1:?usage: paqet_ns.sh PATH-TO-kariz}")
WORK=$(mktemp -d)
ENTRY="kariz-paqet-entry-$$"
EXIT="kariz-paqet-exit-$$"
VE="kpe$$"
VX="kpx$$"
A=192.0.2.21
B=192.0.2.22
PORT=31080
FORWARD=18080
TARGET=18081

cleanup() {
    local result=$?
    if (( result != 0 )); then
        for log in "$WORK"/*.log; do
            if [[ -f $log ]]; then
                printf '\n--- %s ---\n' "$log"
                tail -n 100 "$log"
            fi
        done
    fi
    for ns in "$ENTRY" "$EXIT"; do
        ip netns pids "$ns" 2>/dev/null | xargs -r kill 2>/dev/null || true
        ip netns del "$ns" 2>/dev/null || true
    done
    ip link del "$VE" 2>/dev/null || true
    rm -rf -- "$WORK"
}
trap cleanup EXIT

ip netns add "$ENTRY"
ip netns add "$EXIT"
ip link add "$VE" type veth peer name "$VX"
ip link set "$VE" netns "$ENTRY"
ip link set "$VX" netns "$EXIT"
ip -n "$ENTRY" addr add "$A/24" dev "$VE"
ip -n "$EXIT" addr add "$B/24" dev "$VX"
ip -n "$ENTRY" link set "$VE" up
ip -n "$EXIT" link set "$VX" up
ip -n "$ENTRY" link set lo up
ip -n "$EXIT" link set lo up

entry_fw() { ip netns exec "$ENTRY" iptables -w "$@"; }
exit_fw() { ip netns exec "$EXIT" iptables -w "$@"; }
entry_fw -A OUTPUT -s "$A" -d "$B" -p tcp --sport "$PORT" --tcp-flags RST RST -j DROP
exit_fw -A OUTPUT -s "$B" -d "$A" -p tcp --dport "$PORT" --tcp-flags RST RST -j DROP
entry_fw -A INPUT -s "$B" -d "$A" -p tcp --dport "$PORT" --tcp-flags PSH,ACK PSH,ACK \
    -m comment --comment paqet-carrier-in -j ACCEPT
exit_fw -A INPUT -s "$A" -d "$B" -p tcp --sport "$PORT" --tcp-flags PSH,ACK PSH,ACK \
    -m comment --comment paqet-carrier-out -j ACCEPT
entry_fw -A INPUT -p udp --dport "$PORT" -j DROP
exit_fw -A INPUT -p udp --sport "$PORT" -j DROP

counter() {
    "$1" -nvx -L INPUT | awk -v marker="$2" 'index($0, marker) {sum += $1} END {print sum+0}'
}

token=$("$BIN" token)
wrong=$("$BIN" token)
cat >"$WORK/entry.toml" <<EOF
role = "entry"
mode = "reverse"
[tunnel]
transport = "paqet"
listen = "$A:$PORT"
token = "$token"
[tunnel.kcp]
mtu = 1200
fec_data = 4
fec_parity = 2
[[forward]]
listen = "127.0.0.1:$FORWARD"
target = "127.0.0.1:$TARGET"
protocol = "both"
EOF
write_exit() {
    cat >"$WORK/exit.toml" <<EOF
role = "exit"
mode = "reverse"
[tunnel]
transport = "paqet"
remote = "$A:$PORT"
token = "$1"
[tunnel.kcp]
mtu = 1200
fec_data = 4
fec_parity = 2
EOF
}

cat >"$WORK/echo.py" <<'PY'
import socketserver
import sys
import threading


class TcpEcho(socketserver.BaseRequestHandler):
    def handle(self):
        while data := self.request.recv(8192):
            self.request.sendall(data)


class UdpEcho(socketserver.BaseRequestHandler):
    def handle(self):
        data, sock = self.request
        sock.sendto(data, self.client_address)


address = ("127.0.0.1", int(sys.argv[1]))
with socketserver.ThreadingTCPServer(address, TcpEcho) as tcp, socketserver.UDPServer(address, UdpEcho) as udp:
    threading.Thread(target=udp.serve_forever, daemon=True).start()
    tcp.serve_forever()
PY

cat >"$WORK/client.py" <<'PY'
import socket
import sys
import time


port = int(sys.argv[1])
protocol = sys.argv[2]
deadline = time.monotonic() + float(sys.argv[3])
payload = bytes(range(256)) * (512 if protocol == "tcp" else 2)
kind = socket.SOCK_STREAM if protocol == "tcp" else socket.SOCK_DGRAM
last_error = None
while time.monotonic() < deadline:
    try:
        with socket.socket(socket.AF_INET, kind) as conn:
            conn.settimeout(1)
            conn.connect(("127.0.0.1", port))
            conn.sendall(payload)
            received = bytearray()
            while len(received) < len(payload):
                data = conn.recv(min(8192, len(payload) - len(received)))
                if not data:
                    raise OSError("connection ended before the echo")
                received.extend(data)
            if received != payload:
                raise OSError("echo differed from the payload")
            sys.exit(0)
    except OSError as error:
        last_error = error
        time.sleep(0.15)
sys.exit(str(last_error))
PY

client() {
    ip netns exec "$ENTRY" python3 "$WORK/client.py" "$FORWARD" "$@"
}

ip netns exec "$EXIT" python3 "$WORK/echo.py" "$TARGET" >"$WORK/echo.log" 2>&1 &
ip netns exec "$ENTRY" "$BIN" run -c "$WORK/entry.toml" >"$WORK/entry.log" 2>&1 &
write_exit "$wrong"
ip netns exec "$EXIT" "$BIN" run -c "$WORK/exit.toml" >"$WORK/wrong.log" 2>&1 &
wrong_pid=$!
if client tcp 4; then
    echo "Paqet forwarded traffic with the wrong token" >&2
    exit 1
fi
test "$(counter entry_fw paqet-carrier-in)" -gt 0
test "$(counter exit_fw paqet-carrier-out)" -eq 0
kill "$wrong_pid"
wait "$wrong_pid" || true

write_exit "$token"
ip netns exec "$EXIT" "$BIN" run -c "$WORK/exit.toml" >"$WORK/exit.log" 2>&1 &
client tcp 15
client udp 15
test "$(counter exit_fw paqet-carrier-out)" -gt 0
echo "Paqet forwarded TCP and UDP inside raw TCP segments and rejected the wrong token"
