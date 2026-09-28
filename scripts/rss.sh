#!/usr/bin/env bash
# Memory of a Kariz pair on localhost (direct mode): resident set size (RSS) of each side
# when idle, and with N idle user connections (TCP) or flows (UDP) open through the
# tunnel.
#
# Usage: scripts/rss.sh [transport] [connections] [kariz binary] [tcp|udp]
#   transport: tcp | tcpmux (default) | ws | quic | kcp
# Needs python3 (it plays the target server, which accepts and holds connections, and
# takes UDP packets).
set -euo pipefail

transport=${1:-tcpmux}
n=${2:-100}
kariz=${3:-target/release/kariz}
proto=${4:-tcp}
dir=$(mktemp -d)
pids=()
cleanup() {
    kill "${pids[@]}" 2>/dev/null || true
    rm -rf "$dir"
}
trap cleanup EXIT

read -r p_tunnel p_user p_target < <(python3 -c '
import socket
ports = []
socks = [socket.socket() for _ in range(3)]
for s in socks:
    s.bind(("127.0.0.1", 0))
    ports.append(s.getsockname()[1])
print(*ports)')

python3 -c '
import socket, sys, threading
port = int(sys.argv[1])
u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
u.bind(("127.0.0.1", port))
threading.Thread(target=lambda: [u.recvfrom(65536) for _ in iter(int, 1)], daemon=True).start()
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", port))
s.listen(4096)
held = []
while True:
    held.append(s.accept()[0])' "$p_target" &
pids+=($!)

token=$("$kariz" token)
cat > "$dir/exit.toml" <<CONF
role = "exit"
mode = "direct"
[tunnel]
transport = "$transport"
listen = "127.0.0.1:$p_tunnel"
token = "$token"
[log]
level = "warn"
CONF
cat > "$dir/entry.toml" <<CONF
role = "entry"
mode = "direct"
[tunnel]
transport = "$transport"
remote = "127.0.0.1:$p_tunnel"
token = "$token"
[[forward]]
listen = "127.0.0.1:$p_user"
target = "127.0.0.1:$p_target"
protocol = "$proto"
[log]
level = "warn"
CONF

"$kariz" run -c "$dir/exit.toml" &
exit_pid=$!
pids+=("$exit_pid")
sleep 0.5
"$kariz" run -c "$dir/entry.toml" &
entry_pid=$!
pids+=("$entry_pid")
sleep 1.5

rss() { awk '/^VmRSS/ { print $2 }' "/proc/$1/status"; }
kib() { awk -v k="$1" 'BEGIN { printf "%.1f MiB", k / 1024 }'; }

entry_idle=$(rss "$entry_pid")
exit_idle=$(rss "$exit_pid")
echo "transport $transport ($proto), idle:          entry $(kib "$entry_idle"), exit $(kib "$exit_idle")"

for _ in $(seq "$n"); do
    # Each /dev/udp open is a new client port, so a new flow.
    exec {fd}<>"/dev/$proto/127.0.0.1/$p_user"
    printf 'x' >&"$fd"
done
sleep 2

entry_busy=$(rss "$entry_pid")
exit_busy=$(rss "$exit_pid")
echo "transport $transport ($proto), $n idle conns: entry $(kib "$entry_busy") (+$(kib $((entry_busy - entry_idle)))), exit $(kib "$exit_busy") (+$(kib $((exit_busy - exit_idle))))"
