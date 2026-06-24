#!/usr/bin/env python3
"""Normalize a raw-counts table into an expression table.

Reads a RawCounts TSV (a ``sample`` column followed by gene columns of integer
counts) and writes an ExpressionTable TSV where every count ``x`` becomes
``log2(x + 1)``. Deterministic, offline, stdlib only.

This is an AgentFlow *producer* tool whose ExpressionTable output feeds another
tool's declared ExpressionTable input. It exists so the autonomous loop can
backward-chain: given only RawCounts + a tool that needs an ExpressionTable, the
agent drafts this producer to bridge the type gap. Example artifact, not core.

log2(x + 1) is monotonic, so a downstream median split over a gene yields the
same two groups it would over the raw counts.
"""

import math
import os
import sys


def require_env(name):
    value = os.environ.get(name)
    if not value:
        raise SystemExit(f"missing required environment variable {name}")
    return value


def main():
    counts_path = require_env("AGENTFLOW_INPUT_COUNTS")
    output_path = require_env("AGENTFLOW_OUTPUT_EXPRESSION")

    with open(counts_path) as handle:
        rows = [line.rstrip("\r\n").split("\t") for line in handle if line.strip()]
    if not rows:
        raise SystemExit("counts table is empty")

    header, data = rows[0], rows[1:]
    if not header or header[0] != "sample":
        raise SystemExit("counts table must start with a sample column")

    with open(output_path, "w") as handle:
        handle.write("\t".join(header) + "\n")
        for row in data:
            out = [row[0]]
            for cell in row[1:]:
                try:
                    count = float(cell)
                except ValueError:
                    raise SystemExit(f"non-numeric count {cell!r} for sample {row[0]}")
                if count < 0:
                    raise SystemExit(f"negative count {count} for sample {row[0]}")
                out.append(f"{math.log2(count + 1.0):.4f}")
            handle.write("\t".join(out) + "\n")

    sys.stderr.write(
        f"counts_to_expression ok: log2(x+1) over {len(data)} samples, "
        f"{len(header) - 1} gene(s)\n"
    )


if __name__ == "__main__":
    main()
