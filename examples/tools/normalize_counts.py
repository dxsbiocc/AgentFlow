#!/usr/bin/env python3
"""Library-size normalize a RawCounts table into a NormalizedCounts table.

Reads a RawCounts TSV (``sample`` column then integer gene-count columns) and
writes a NormalizedCounts TSV where each count is scaled to counts-per-million
of that sample's total (CPM). Deterministic, offline, stdlib only.

A fixture *producer* tool. Its NormalizedCounts output is itself consumed by
another producer (normalized_to_expression), so the autonomous loop must chain
two producers to reach an ExpressionTable from RawCounts — exercising
multi-level backward chaining. Example artifact, not core.
"""

import os
import sys


def require_env(name):
    value = os.environ.get(name)
    if not value:
        raise SystemExit(f"missing required environment variable {name}")
    return value


def main():
    counts_path = require_env("AGENTFLOW_INPUT_COUNTS")
    output_path = require_env("AGENTFLOW_OUTPUT_NORMALIZED")

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
            counts = []
            for cell in row[1:]:
                try:
                    counts.append(float(cell))
                except ValueError:
                    raise SystemExit(f"non-numeric count {cell!r} for sample {row[0]}")
            total = sum(counts) or 1.0
            scaled = [f"{count / total * 1_000_000.0:.4f}" for count in counts]
            handle.write("\t".join([row[0], *scaled]) + "\n")

    sys.stderr.write(
        f"normalize_counts ok: CPM over {len(data)} samples, {len(header) - 1} gene(s)\n"
    )


if __name__ == "__main__":
    main()
