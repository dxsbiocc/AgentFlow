#!/usr/bin/env python3
"""Local survival-association analysis for one gene (stdlib only, offline).

Reads imported expression and survival TSV files, joins them by sample, splits
the selected gene at its median expression, runs a two-group log-rank test, and
emits a marker_report-format Markdown file for AgentFlow observers.
"""

import math
import os
import sys


def median(xs):
    s = sorted(xs)
    n = len(s)
    if n == 0:
        return float("nan")
    return s[n // 2] if n % 2 else (s[n // 2 - 1] + s[n // 2]) / 2


def median_survival(records):
    times = sorted(t for t, _ in records)
    return median(times) if times else float("nan")


def logrank(group_hi, group_lo):
    """Two-group log-rank. records: list of (time, event_bool). Returns (chi2, p, o1, e1)."""
    labeled = [(t, e, 1) for t, e in group_hi] + [(t, e, 0) for t, e in group_lo]
    event_times = sorted({t for t, e, _ in labeled if e})
    o1 = e1 = v = 0.0
    for t in event_times:
        at_risk = [(tt, ee, gg) for tt, ee, gg in labeled if tt >= t]
        n = len(at_risk)
        n1 = sum(1 for _, _, g in at_risk if g == 1)
        d = sum(1 for tt, ee, _ in at_risk if ee and tt == t)
        d1 = sum(1 for tt, ee, g in at_risk if ee and tt == t and g == 1)
        if n <= 1:
            continue
        o1 += d1
        e1 += d * n1 / n
        v += d * (n1 / n) * (1 - n1 / n) * (n - d) / (n - 1)
    if v <= 0:
        return 0.0, 1.0, o1, e1
    chi2 = (o1 - e1) ** 2 / v
    p = math.erfc(math.sqrt(chi2 / 2.0))
    return chi2, p, o1, e1


def require_env(name):
    value = os.environ.get(name)
    if not value:
        raise SystemExit(f"{name} is required")
    return value


def read_tsv(path):
    with open(path, newline="") as handle:
        rows = [line.rstrip("\r\n").split("\t") for line in handle if line.strip()]
    if not rows:
        raise SystemExit(f"{path} is empty")
    return rows[0], rows[1:]


def read_expression(path, gene):
    header, rows = read_tsv(path)
    if not header or header[0] != "sample":
        raise SystemExit("expression table must start with a sample column")
    if gene not in header:
        raise SystemExit(f"gene column {gene} not found in expression table")
    gene_idx = header.index(gene)
    values = {}
    for line_no, row in enumerate(rows, start=2):
        if len(row) <= gene_idx:
            raise SystemExit(f"expression table row {line_no} is missing {gene}")
        sample = row[0]
        if not sample:
            raise SystemExit(f"expression table row {line_no} has an empty sample id")
        try:
            values[sample] = float(row[gene_idx])
        except ValueError as exc:
            raise SystemExit(
                f"expression value for {gene} in sample {sample} is not numeric"
            ) from exc
    return values


def read_survival(path):
    header, rows = read_tsv(path)
    required = {"sample", "time", "status"}
    missing = sorted(required.difference(header))
    if missing:
        raise SystemExit(f"survival table missing required column(s): {', '.join(missing)}")
    sample_idx = header.index("sample")
    time_idx = header.index("time")
    status_idx = header.index("status")
    values = {}
    for line_no, row in enumerate(rows, start=2):
        if len(row) <= max(sample_idx, time_idx, status_idx):
            raise SystemExit(f"survival table row {line_no} is incomplete")
        sample = row[sample_idx]
        if not sample:
            raise SystemExit(f"survival table row {line_no} has an empty sample id")
        try:
            time = float(row[time_idx])
            status = int(row[status_idx])
        except ValueError as exc:
            raise SystemExit(
                f"survival value for sample {sample} has non-numeric time/status"
            ) from exc
        if time < 0:
            raise SystemExit(f"survival time for sample {sample} is negative")
        if status not in (0, 1):
            raise SystemExit(f"survival status for sample {sample} must be 0 or 1")
        values[sample] = (time, bool(status))
    return values


def score_from_p(p, hi_worse):
    if p <= 0:
        magnitude = float("inf")
    else:
        magnitude = -math.log10(p)
    return (1 if hi_worse else -1) * magnitude


def main():
    expression_path = require_env("AGENTFLOW_INPUT_EXPRESSION_TABLE")
    survival_path = require_env("AGENTFLOW_INPUT_SURVIVAL_TABLE")
    gene = require_env("AGENTFLOW_PARAM_GENE")
    output_path = require_env("AGENTFLOW_OUTPUT_REPORT")

    expr = read_expression(expression_path, gene)
    surv = read_survival(survival_path)
    paired = [
        (expr[sample], surv[sample][0], surv[sample][1])
        for sample in expr
        if sample in surv
    ]
    if len(paired) < 6:
        raise SystemExit(f"too few joined samples ({len(paired)}) for {gene}; need at least 6")

    cut = median([value for value, _, _ in paired])
    hi = [(time, event) for value, time, event in paired if value >= cut]
    lo = [(time, event) for value, time, event in paired if value < cut]
    if not hi or not lo:
        raise SystemExit("median split produced an empty expression group")

    chi2, p, _, _ = logrank(hi, lo)
    med_hi, med_lo = median_survival(hi), median_survival(lo)
    hi_worse = med_hi < med_lo
    direction = (
        "high-expression associated with worse overall survival"
        if hi_worse
        else "high-expression associated with better overall survival"
    )
    signed = score_from_p(p, hi_worse)

    lines = [
        "Marker report",
        f"Gene: {gene}",
        f"score: {signed:.3f}",
        f"n: {len(paired)}  high: {len(hi)}  low: {len(lo)}",
        "events: "
        f"high={sum(1 for _, event in hi if event)} "
        f"low={sum(1 for _, event in lo if event)}",
        f"median_OS: high={med_hi:.2f} low={med_lo:.2f}",
        f"logrank_chi2: {chi2:.4f}",
        f"logrank_p: {p:.6g}",
        f"direction: {direction}",
        "source: local imported expression+survival cohort",
    ]
    with open(output_path, "w") as handle:
        handle.write("\n".join(lines) + "\n")
    sys.stderr.write(f"local_survival_assoc ok: {gene} p={p:.4g} {direction}\n")


if __name__ == "__main__":
    main()
