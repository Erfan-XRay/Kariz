#!/usr/bin/env bash
# TEMPORARY (issue #73, removed before merge): an idle reverse tunnel over tcpmux and over
# wss, with mux pings every second and every control socket asked for its status 20 times
# a second (an agent asks every 2 s), for MINUTES; prints each process's memory as it goes.
set -euo pipefail

BIN=$(readlink -f "${1:?usage: soak_idle.sh PATH-TO-kariz [MINUTES]}")
MINUTES=${2:-10}
W=$(mktemp -d)
cd "$W"
PIDS=()
cleanup() {
    for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
}
trap cleanup EXIT

TOKEN=$("$BIN" token | tail -1)
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 2 \
    -subj /CN=kariz -keyout key.pem -out cert.pem 2>/dev/null
PIN=$("$BIN" pin cert.pem | tail -1)

pair() { # name transport port front tls-entry tls-exit
    cat >"entry-$1.toml" <<EOF
role = "entry"
mode = "reverse"
[tunnel]
transport = "$2"
listen = "127.0.0.1:$3"
token = "$TOKEN"
$5
[tuning]
keepalive_secs = 1
[[forward]]
listen = "127.0.0.1:$4"
target = "127.0.0.1:1"
protocol = "tcp+udp"
[log]
level = "warn"
EOF
    cat >"exit-$1.toml" <<EOF
role = "exit"
mode = "reverse"
[tunnel]
transport = "$2"
remote = "127.0.0.1:$3"
token = "$TOKEN"
$6
[tuning]
keepalive_secs = 1
[log]
level = "warn"
EOF
}
pair tcp tcpmux 39080 39443 "" ""
pair wss wss 39180 39543 "[tunnel.tls]
cert = \"$W/cert.pem\"
key = \"$W/key.pem\"" "[tunnel.tls]
pin_sha256 = \"$PIN\""

declare -A PID
for side in entry-tcp exit-tcp entry-wss exit-wss; do
    "$BIN" run -c "$side.toml" >"$side.log" 2>&1 &
    PID[$side]=$!
    PIDS+=($!)
    sleep 0.5
done
sleep 5

# 20 status requests a second on each control socket.
python3 - "$W" <<'EOF' &
import socket, sys, time
w = sys.argv[1]
socks = [f"{w}/{s}.sock" for s in ("entry-tcp", "exit-tcp", "entry-wss", "exit-wss")]
n = 0
while True:
    for path in socks:
        try:
            c = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            c.settimeout(2)
            c.connect(path)
            c.sendall(b"status\n")
            while c.recv(65536):
                pass
            c.close()
            n += 1
        except OSError:
            pass
    time.sleep(0.05)
EOF
PIDS+=($!)

rss() { awk '/^VmRSS/ {print $2}' "/proc/$1/status"; }
printf '%6s %12s %12s %12s %12s\n' secs entry-tcp exit-tcp entry-wss exit-wss
for ((t = 0; t <= MINUTES * 60; t += 30)); do
    printf '%6d %12s %12s %12s %12s\n' "$t" \
        "$(rss "${PID[entry-tcp]}")" "$(rss "${PID[exit-tcp]}")" \
        "$(rss "${PID[entry-wss]}")" "$(rss "${PID[exit-wss]}")"
    sleep 30
done
"$BIN" status -c entry-wss.toml || true
tail -5 entry-wss.log exit-wss.log || true
