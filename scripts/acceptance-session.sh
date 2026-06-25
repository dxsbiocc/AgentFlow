#!/usr/bin/env bash
# End-to-end acceptance for the autonomous research loop: from a single
# hypothesis + raw inputs, the agent builds a multi-step flow itself, runs it
# (optionally in parallel), foraged literature is graded honestly by source
# trust, and the deterministic verdict refuses to over-claim. Exercises the
# v0.3.1 work (multi-level chaining, parallel execution, preprint/retraction
# grading) on top of the unchanged 0-LLM verdict core.
#
# Usage: scripts/acceptance-session.sh   (run from the repo root)
# Exits non-zero on the first failed check.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

BIN="${AGENTFLOW_BIN:-$ROOT/target/debug/agentflow}"
if [ ! -x "$BIN" ]; then
  echo "building agentflow CLI..."
  cargo build -q -p agentflow-cli
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/af-acceptance-XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

pass=0
fail=0
check() { # check <description> <0|1 condition-result>
  if [ "$2" = "0" ]; then
    echo "  [PASS] $1"; pass=$((pass + 1))
  else
    echo "  [FAIL] $1"; fail=$((fail + 1))
  fi
}
jq_id() { python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])'; }

echo "== Stage 0: setup =="
"$BIN" init --name Acceptance --path "$WORK" >/dev/null
# A two-producer ladder + the consumer; NO single-step shortcut tool exists, so
# reaching an ExpressionTable from RawCounts requires chaining two producers.
"$BIN" tools register examples/tools/normalize_counts.tool.yaml --path "$WORK" >/dev/null
"$BIN" tools register examples/tools/normalized_to_expression.tool.yaml --path "$WORK" >/dev/null
"$BIN" tools register examples/tools/local_survival_assoc.tool.yaml --path "$WORK" >/dev/null
# Import ONLY RawCounts + SurvivalTable (no ExpressionTable, no NormalizedCounts).
"$BIN" import examples/data/lihc_demo/counts.tsv --type RawCounts --mode copy --path "$WORK" >/dev/null
"$BIN" import examples/data/lihc_demo/survival.tsv --type SurvivalTable --mode copy --path "$WORK" >/dev/null
HID="$("$BIN" hypothesis create --statement "SPP1 expression associates with overall survival in the imported cohort" --origin user_goal --goal g1 --json --path "$WORK" | jq_id)"
echo "  hypothesis=$HID"

echo "== Stage 1: autonomous multi-level chaining + parallel execution =="
RUN="$("$BIN" agent run --apply --auto-run --no-auto-synth --no-auto-forage --no-semantic-match --max-parallel 4 --path "$WORK" 2>&1)"
echo "$RUN" | grep -q "step_normalize_counts ran" && r=0 || r=1
check "agent ran the deepest producer (normalize_counts)" "$r"
echo "$RUN" | grep -q "step_normalized_to_expression ran" && r=0 || r=1
check "agent ran the middle producer (normalized_to_expression)" "$r"
echo "$RUN" | grep -q "step_survival_assoc ran and observed" && r=0 || r=1
check "agent ran the consumer and recorded an observation" "$r"
echo "$RUN" | grep -q "Outcome: handed_off" && r=0 || r=1
check "agent handed off (did NOT autonomously affirm)" "$r"
INSPECT="$("$BIN" flow inspect "auto_$HID" --path "$WORK")"
echo "$INSPECT" | grep -q "Steps: 3" && r=0 || r=1
check "auto-built flow has 3 chained steps" "$r"
echo "$INSPECT" | grep -q "Edges: 2" && r=0 || r=1
check "auto-built flow has 2 dependency edges" "$r"
REPORT="$(cat "$WORK"/.agentflow/artifacts/computed/*/step_survival_assoc_report)"
echo "$REPORT" | grep -qi "Gene: SPP1" && echo "$REPORT" | grep -qi "logrank_p" && r=0 || r=1
check "real analysis produced an SPP1 log-rank marker report" "$r"
OBS="$("$BIN" observations list --json --path "$WORK" | python3 -c 'import sys,json; d=json.load(sys.stdin); print((d["observations"] if isinstance(d,dict) else d)[0]["id"])')"

echo "== Stage 2: evidence breadth + honest grading =="
# Human-in-the-loop confirms the computed observation supports the hypothesis.
"$BIN" evidence link --hypothesis "$HID" --observation "$OBS" --stance supports --grade observed --note "computed log-rank" --path "$WORK" >/dev/null
PEER="$("$BIN" forage observe --source pubmed --external-id "PMID:30000001" --title "peer-reviewed" --access open_access_full_text --json --path "$WORK" | jq_id)"
PRE="$("$BIN" forage observe --source biorxiv --external-id "doi:10.1101/2026.02.02" --title "preprint" --access user_provided_full_text --json --path "$WORK" | jq_id)"
RET="$("$BIN" forage observe --source pubmed --external-id "PMID:29999999" --title "retracted" --access open_access_full_text --retracted --json --path "$WORK" | jq_id)"
"$BIN" forage link --hypothesis "$HID" --observation "$PEER" --stance supports --note peer --path "$WORK" >/dev/null
"$BIN" forage link --hypothesis "$HID" --observation "$PRE" --stance supports --note preprint --path "$WORK" >/dev/null
"$BIN" forage link --hypothesis "$HID" --observation "$RET" --stance supports --note retracted --path "$WORK" >/dev/null
EV="$("$BIN" evidence list --hypothesis "$HID" --json --path "$WORK")"
# Grade-cap: an autonomously inferred gene caps the computed result below observed.
echo "$EV" | grep -q '"grade":"inferred"' && r=0 || r=1
check "computed result is grade-capped to inferred (autonomous gene)" "$r"
echo "$EV" | grep -q '"grade":"literature_supported"' && r=0 || r=1
check "peer-reviewed full text -> literature_supported" "$r"
echo "$EV" | grep -q '"grade":"hypothesis"' && r=0 || r=1
check "bioRxiv preprint full text -> hypothesis (capped)" "$r"
echo "$EV" | grep -q '"grade":"unsupported"' && r=0 || r=1
check "retracted source -> unsupported" "$r"

echo "== Stage 3: honest deterministic verdict =="
V1="$("$BIN" verdict render --hypothesis "$HID" --json --path "$WORK")"
echo "$V1" | grep -q '"verdict":"inconclusive"' && r=0 || r=1
check "verdict is inconclusive (refuses to affirm without observed support)" "$r"
echo "$V1" | grep -q 'has_obs_support=false' && r=0 || r=1
check "rationale names the missing ingredient (observed support)" "$r"
H1="$(echo "$V1" | python3 -c 'import sys,json,hashlib; d=json.load(sys.stdin); d.pop("created_at",None); print(hashlib.sha256(json.dumps(d,sort_keys=True).encode()).hexdigest())')"
H2="$("$BIN" verdict render --hypothesis "$HID" --json --path "$WORK" | python3 -c 'import sys,json,hashlib; d=json.load(sys.stdin); d.pop("created_at",None); print(hashlib.sha256(json.dumps(d,sort_keys=True).encode()).hexdigest())')"
[ "$H1" = "$H2" ] && r=0 || r=1
check "verdict render is deterministic (identical across renders)" "$r"

echo ""
echo "acceptance: $pass passed, $fail failed"
[ "$fail" -eq 0 ] || exit 1
echo "AgentFlow session acceptance passed."
