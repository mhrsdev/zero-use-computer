#!/usr/bin/env bash
# All measured runs: N per arm, alternating which arm goes first (ABBA), each
# from a clean start (run_one.sh), then the analysis.
#
#   run_all.sh N RESULTS_DIR
set -uo pipefail
n="$1"
out="$2"
here="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$out/raw"
for i in $(seq 1 "$n"); do
  if [ $((i % 2)) -eq 1 ]; then order="baseline current"; else order="current baseline"; fi
  for arm in $order; do
    echo "== $arm-$i $(date -u +%H:%M:%S)"
    "$here/run_one.sh" "$arm" "$out/raw/$arm-$i" 2>&1 | tail -1
  done
done
python3 "$here/analyze.py" "$out" >/dev/null && echo "summary: $out/summary.md"
