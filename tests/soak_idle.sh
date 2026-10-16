#!/usr/bin/env bash
# TEMPORARY (issue #73, removed before merge): idle reverse tunnels over tcpmux, wss, kcp
# and quic, with mux pings every second and every control socket asked for its status 20
# times a second (an agent asks every 2 s), for MINUTES; prints each process's memory (KB).
set -euo pipefail

BIN=$(readlink -f "${1:?usage: soak_idle.sh PATH-TO-kariz [MINUTES] [LABEL] [PORT-OFFSET]}")
MINUTES=${2:-10}
LABEL=${3:-$("$BIN" --version 2>/dev/null || echo kariz)}
OFF=${4:-0}
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
pair tcp tcpmux $((39080 + OFF)) $((39443 + OFF)) "" ""
pair wss wss $((39180 + OFF)) $((39543 + OFF)) "[tunnel.tls]
cert = \"$W/cert.pem\"
key = \"$W/key.pem\"" "[tunnel.tls]
pin_sha256 = \"$PIN\""
pair kcp kcp $((39280 + OFF)) $((39643 + OFF)) "" ""
pair quic quic $((39380 + OFF)) $((39743 + OFF)) "" ""

SIDES=()
declare -A PID
for name in tcp wss kcp quic; do
    for role in entry exit; do
        side="$role-$name"
        if ! "$BIN" check -c "$side.toml" >/dev/null 2>&1; then
            echo "($side: this build cannot run it, skipped)"
            continue
        fi
        "$BIN" run -c "$side.toml" >"$side.log" 2>&1 &
        PID[$side]=$!
        PIDS+=($!)
        SIDES+=("$side")
        sleep 0.5
    done
done
sleep 5

# 20 status requests a second on each control socket.
python3 - "$W" "${SIDES[@]}" <<'EOF' &
import socket, sys, time
w, sides = sys.argv[1], sys.argv[2:]
while True:
    for side in sides:
        try:
            c = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            c.settimeout(2)
            c.connect(f"{w}/{side}.sock")
            c.sendall(b"status\n")
            while c.recv(65536):
                pass
            c.close()
        except OSError:
            pass
    time.sleep(0.05)
EOF
PIDS+=($!)

rss() { awk '/^VmRSS/ {print $2}' "/proc/$1/status" 2>/dev/null || echo dead; }
echo "== $LABEL"
printf '%6s' secs
for side in "${SIDES[@]}"; do printf ' %10s' "$side"; done
echo
for ((t = 0; t <= MINUTES * 60; t += 60)); do
    printf '%6d' "$t"
    for side in "${SIDES[@]}"; do printf ' %10s' "$(rss "${PID[$side]}")"; done
    echo
    sleep 60
done
for side in "${SIDES[@]}"; do
    echo "-- $side: $(grep -c . "$side.log" || true) log lines, last: $(tail -n 1 "$side.log" | cut -c1-160)"
done
