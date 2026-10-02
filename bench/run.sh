#!/usr/bin/env bash
# Run the agent benchmark (crates/computer-use/examples/agent_bench) inside a
# throwaway headless desktop: Xvfb, a private D-Bus session and the AT-SPI
# bus. Each run starts its own fixture app (bench/fixtures/bench_app.py).
#
#   bench/run.sh --scripted --label v3.2            # no model, estimates
#   ANTHROPIC_API_KEY=… bench/run.sh --runs 5        # the real model
#
# Options go to the benchmark; bench/README.md lists them.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
cargo build -q --release -p computer-use --example agent_bench
export GTK_A11Y=atspi
SETTLE="${SETTLE:-1}" APPS="" CU_DISPLAY="${CU_DISPLAY:-:97}" \
  CU_SCREEN="${CU_SCREEN:-1280x1024x24}" \
  scripts/desktop-session.sh "$root/target/release/examples/agent_bench" "$@"
