#!/usr/bin/env python3
"""Select a subset of gene columns from an imported expression table.

Reads an ExpressionTable TSV (a ``sample`` column followed by gene columns),
keeps only the gene columns named in the ``genes`` parameter, and writes a new
ExpressionTable TSV. Deterministic, offline, stdlib only.

This is an AgentFlow *producer* tool: its output feeds another tool's declared
input, exercising flow composition + per-step I/O staging. It is an example
artifact, not core engine code.
"""

import os
import sys


def require_env(name):
    value = os.environ.get(name)
    if not value:
        raise SystemExit(f"missing required environment variable {name}")
    return value


def read_tsv(path):
    with open(path) as handle:
        rows = [line.rstrip("\r\n").split("\t") for line in handle if line.strip()]
    if not rows:
        raise SystemExit("expression table is empty")
    return rows[0], rows[1:]


def main():
    expression_path = require_env("AGENTFLOW_INPUT_EXPRESSION_TABLE")
    output_path = require_env("AGENTFLOW_OUTPUT_SELECTED")
    genes = [g.strip() for g in require_env("AGENTFLOW_PARAM_GENES").split(",") if g.strip()]
    if not genes:
        raise SystemExit("genes parameter must name at least one gene column")

    header, rows = read_tsv(expression_path)
    if not header or header[0] != "sample":
        raise SystemExit("expression table must start with a sample column")

    missing = [g for g in genes if g not in header]
    if missing:
        raise SystemExit(f"gene column(s) not found in expression table: {', '.join(missing)}")

    keep_idx = [0] + [header.index(g) for g in genes]
    out_header = [header[i] for i in keep_idx]

    with open(output_path, "w") as handle:
        handle.write("\t".join(out_header) + "\n")
        for row in rows:
            handle.write("\t".join(row[i] if i < len(row) else "" for i in keep_idx) + "\n")

    sys.stderr.write(f"expression_select ok: kept {len(genes)} gene(s) over {len(rows)} samples\n")


if __name__ == "__main__":
    main()
