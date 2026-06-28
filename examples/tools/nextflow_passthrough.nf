nextflow.enable.dsl = 2

process agentflow_passthrough {
    script:
    '''
    set -euo pipefail
    cp "$AGENTFLOW_INPUT_TABLE" "$AGENTFLOW_OUTPUT_TABLE"
    '''
}

workflow {
    agentflow_passthrough()
}
