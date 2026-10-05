#!/usr/bin/env bash
# Run one fuzz target for a bounded time: fuzz/run.sh <target> [seconds] (default 600).
# New corpus entries go to fuzz/work/<target> (not committed); the committed seeds in fuzz/corpus/<target> are read only.
# On a finding the input is left in fuzz/artifacts/<target>/; copy a minimised one into the crate's tests/fuzz_regressions.
set -euo pipefail
cd "$(dirname "$0")"
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
t="${1:?target}"; secs="${2:-600}"
mkdir -p "work/$t"
exec cargo fuzz run "$t" "work/$t" "corpus/$t" -- -max_total_time="$secs" -rss_limit_mb=2048 -timeout=10 -print_final_stats=1
