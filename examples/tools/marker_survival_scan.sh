cat "$AGENTFLOW_INPUT_EXPRESSION_TABLE" >/dev/null
cat "$AGENTFLOW_INPUT_SURVIVAL_TABLE" >/dev/null
printf "Marker report\nGene: %s\nscore: 0.61\n" "$AGENTFLOW_PARAM_GENE" > "$AGENTFLOW_OUTPUT_REPORT"
echo marker scan ok
