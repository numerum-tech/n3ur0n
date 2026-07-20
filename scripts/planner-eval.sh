#!/usr/bin/env bash
# Planner accuracy suite — run often to see how the planner behaves.
# Usage: scripts/planner-eval.sh [model] [runs-per-case]
# Env: PLANNER_EVAL_BASE_URL (default http://localhost:11434),
#      PLANNER_EVAL_API_KEY, PLANNER_EVAL_REPORT (default target/planner-eval-report.json)
set -euo pipefail
cd "$(dirname "$0")/.."
export PLANNER_EVAL_MODEL="${1:-${PLANNER_EVAL_MODEL:-llama3.1:8b}}"
export PLANNER_EVAL_RUNS="${2:-${PLANNER_EVAL_RUNS:-1}}"
export PLANNER_EVAL_BASE_URL="${PLANNER_EVAL_BASE_URL:-http://localhost:11434}"
export PLANNER_EVAL_REPORT="${PLANNER_EVAL_REPORT:-$PWD/target/planner-eval-report.json}"
echo "planner eval · model=$PLANNER_EVAL_MODEL · runs=$PLANNER_EVAL_RUNS · endpoint=$PLANNER_EVAL_BASE_URL"
exec cargo test -p n3ur0n-node --test planner_eval -- --ignored --nocapture
