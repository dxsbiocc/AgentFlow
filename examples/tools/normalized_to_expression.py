#!/usr/bin/env python3
"""Transform a NormalizedCounts table into an ExpressionTable via log2(x+1).

Reads a NormalizedCounts TSV (``sample`` column then numeric gene columns) and
writes an ExpressionTable TSV where every value ``x`` becomes ``log2(x + 1)``.
Deterministic, offline, stdlib only.

A fixture *producer* tool whose RawCounts -> NormalizedCounts predecessor is
normalize_counts. Together they exercise multi-level backward chaining: from
only RawCounts, the autonomous loop must chain normalize_counts then this tool
to feed a consumer that needs an ExpressionTable. Example artifact, not core.
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
    normalized_path = require_env("AGENTFLOW_INPUT_NORMALIZED")
    output_path = require_env("AGENTFLOW_OUTPUT_EXPRESSION")

    with open(normalized_path) as handle:
        rows = [line.rstrip("\r\n").split("\t") for line in handle if line.strip()]
    if not rows:
        raise SystemExit("normalized table is empty")
    header, data = rows[0], rows[1:]
    if not header or header[0] != "sample":
        raise SystemExit("normalized table must start with a sample column")

    with open(output_path, "w") as handle:
        handle.write("\t".join(header) + "\n")
        for row in data:
            out = [row[0]]
            for cell in row[1:]:
                try:
                    value = float(cell)
                except ValueError:
                    raise SystemExit(f"non-numeric value {cell!r} for sample {row[0]}")
                if value < 0:
                    raise SystemExit(f"negative value {value} for sample {row[0]}")
                out.append(f"{math.log2(value + 1.0):.4f}")
            handle.write("\t".join(out) + "\n")

    sys.stderr.write(
        f"normalized_to_expression ok: log2(x+1) over {len(data)} samples\n"
    )


if __name__ == "__main__":
    main()
