#!/usr/bin/env bash
# The x86_64 binaries run on the oldest x86_64 CPUs: QEMU's qemu64 model has none of the
# instructions newer compilers like to assume (SSE4, AVX, BMI, MOVBE). An instruction it
# lacks stops the program with SIGILL, as on old or virtual servers.
set -euo pipefail

DIR=${1:?usage: cpu_baseline.sh DIR-WITH-kariz-AND-kariz-panel}
run() { qemu-x86_64 -cpu qemu64 "$@"; }

run "$DIR/kariz" --version
run "$DIR/kariz" token >/dev/null
run "$DIR/kariz-panel" --version
# The agent opens its state and starts its crypto before it reads the config.
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
code=0
run "$DIR/kariz-panel" agent -c "$work/missing.toml" >/dev/null 2>&1 || code=$?
# 132 is SIGILL (128 + 4); any other ending (a missing config) is fine here.
if (( code == 132 )); then
    echo "kariz-panel agent hit an illegal instruction on a baseline x86_64 CPU" >&2
    exit 1
fi
echo "kariz and kariz-panel run on a baseline x86_64 CPU"
