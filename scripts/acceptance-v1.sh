#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -q -p agentflow-cli -- help >/dev/null

DEMO_DIR="$(mktemp -d "${TMPDIR:-/tmp}/agentflow-acceptance.XXXXXX")"
trap 'rm -rf "$DEMO_DIR"' EXIT

cargo run -q -p agentflow-cli -- init --name AcceptanceV1 --path "$DEMO_DIR" >/dev/null
cargo run -q -p agentflow-cli -- tools register examples/tools/marker_survival_scan.tool.yaml --path "$DEMO_DIR" >/dev/null

expression_import="$(
  cargo run -q -p agentflow-cli -- import examples/data/expression.tsv --type TSV --path "$DEMO_DIR"
)"
survival_import="$(
  cargo run -q -p agentflow-cli -- import examples/data/survival.tsv --type TSV --path "$DEMO_DIR"
)"
expression_id="$(printf '%s\n' "$expression_import" | sed -n 's/^Id: //p')"
survival_id="$(printf '%s\n' "$survival_import" | sed -n 's/^Id: //p')"

if [[ -z "$expression_id" || -z "$survival_id" ]]; then
  echo "failed to extract imported artifact ids" >&2
  exit 1
fi

sed \
  -e "s/artifact_REPLACE_WITH_IMPORTED_ID/$expression_id/g" \
  -e "s/artifact_REPLACE_WITH_IMPORTED_SURVIVAL_ID/$survival_id/g" \
  examples/flows/marker_demo.flow.yaml > "$DEMO_DIR/marker_demo.flow.yaml"

cargo run -q -p agentflow-cli -- flow validate "$DEMO_DIR/marker_demo.flow.yaml" --json --path "$DEMO_DIR" | grep -q '"valid":true'
cargo run -q -p agentflow-cli -- flow approve "$DEMO_DIR/marker_demo.flow.yaml" --path "$DEMO_DIR" >/dev/null

run_output="$(cargo run -q -p agentflow-cli -- run marker_demo --path "$DEMO_DIR")"
printf '%s\n' "$run_output" | grep -q 'Completed steps: 1'
printf '%s\n' "$run_output" | grep -q 'Failed steps: 0'
printf '%s\n' "$run_output" | grep -q ' \[succeeded\] '
run_attempt_id="$(printf '%s\n' "$run_output" | awk '/^attempt_/ {print $1; exit}')"
run_id="$(printf '%s\n' "$run_output" | awk '/^attempt_/ {print $2; exit}')"

if [[ -z "$run_attempt_id" || -z "$run_id" ]]; then
  echo "failed to extract run ids" >&2
  exit 1
fi

cargo run -q -p agentflow-cli -- status --json --path "$DEMO_DIR" | grep -q '"run_attempts":1'
cargo run -q -p agentflow-cli -- runs list --flow marker_demo --json --path "$DEMO_DIR" | grep -q '"schema_version":"agentflow.runs.v0"'
cargo run -q -p agentflow-cli -- runs inspect "$run_id" --json --path "$DEMO_DIR" | grep -q '"schema_version":"agentflow.run_inspection.v0"'
cargo run -q -p agentflow-cli -- runs inspect "$run_attempt_id" --path "$DEMO_DIR" | grep -q '\[succeeded\]'
cargo run -q -p agentflow-cli -- artifacts list --json --path "$DEMO_DIR" | grep -q '"kind":"computed"'
cargo run -q -p agentflow-cli -- observations list --json --path "$DEMO_DIR" | grep -q '"kind":"marker_report"'
cargo run -q -p agentflow-cli -- report marker_demo --path "$DEMO_DIR" | grep -q 'marker_report'
cargo run -q -p agentflow-cli -- cache explain marker_demo.scan --path "$DEMO_DIR" | grep -q 'step:marker_demo/scan \[hit\]'
cargo run -q -p agentflow-cli -- cache list --json --path "$DEMO_DIR" | grep -q '"schema_version":"agentflow.cache_entries.v0"'
cargo run -q -p agentflow-cli -- cache prune --older-than-seconds 31536000 --json --path "$DEMO_DIR" | grep -q '"removed_entries":0'

echo "AgentFlow V1 acceptance gate passed."
