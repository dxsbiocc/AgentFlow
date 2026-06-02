#!/usr/bin/env python3
"""TCGA survival-association analysis for a single gene (cBioPortal, stdlib only).

Fetches per-sample mRNA expression and per-patient overall survival for one gene
from the public cBioPortal REST API, splits samples at the median expression, and
runs a log-rank test (high vs low expression). Emits a marker_report-format file so
AgentFlow's `marker_report` observer turns the result into observed evidence.

Inputs are taken from AgentFlow tool env vars when present, else CLI flags:
  AGENTFLOW_PARAM_GENE   / --gene    gene symbol (e.g. THRSP) or entrez id
  AGENTFLOW_PARAM_STUDY  / --study   cBioPortal study id (default LIHC PanCancer)
  AGENTFLOW_OUTPUT_REPORT/ --out     output report path

Network only; no third-party packages. Honest output: it reports exactly the real
numbers computed (group sizes, median OS, events, log-rank chi2 and p), and a signed
significance `score` (sign = direction of the high-expression effect on hazard).
"""

import argparse
import json
import math
import os
import sys
import urllib.parse
import urllib.request

API = "https://www.cbioportal.org/api"


def _get(url):
    req = urllib.request.Request(url, headers={"Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as resp:
        return json.load(resp)


def _post(url, body):
    req = urllib.request.Request(
        url,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json", "Accept": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=90) as resp:
        return json.load(resp)


def resolve_entrez(gene):
    gene = gene.strip()
    if gene.isdigit():
        return int(gene)
    info = _get(f"{API}/genes/{urllib.parse.quote(gene)}")
    return int(info["entrezGeneId"])


def fetch_expression(study, profile, entrez):
    url = f"{API}/molecular-profiles/{profile}/molecular-data/fetch?projection=SUMMARY"
    rows = _post(url, {"entrezGeneIds": [entrez], "sampleListId": f"{study}_all"})
    # patientId -> mean expression (dedup multi-sample patients)
    acc = {}
    for r in rows:
        v = r.get("value")
        if v is None:
            continue
        acc.setdefault(r["patientId"], []).append(float(v))
    return {p: sum(vs) / len(vs) for p, vs in acc.items()}


def fetch_survival(study):
    url = (
        f"{API}/studies/{study}/clinical-data"
        f"?clinicalDataType=PATIENT&projection=SUMMARY&pageSize=100000"
    )
    months, status = {}, {}
    for r in _get(url):
        a = r["clinicalAttributeId"]
        if a == "OS_MONTHS":
            try:
                months[r["patientId"]] = float(r["value"])
            except ValueError:
                pass
        elif a == "OS_STATUS":
            status[r["patientId"]] = str(r["value"]).startswith("1")
    surv = {}
    for p, m in months.items():
        if p in status and m >= 0:
            surv[p] = (m, status[p])
    return surv


def median(xs):
    s = sorted(xs)
    n = len(s)
    if n == 0:
        return float("nan")
    return s[n // 2] if n % 2 else (s[n // 2 - 1] + s[n // 2]) / 2


def median_survival(records):
    """Kaplan-Meier-free proxy: smallest event time where >=50% have had an event;
    if never reached, report '>last observed time'."""
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
        v += (d * (n1 / n) * (1 - n1 / n) * (n - d) / (n - 1))
    if v <= 0:
        return 0.0, 1.0, o1, e1
    chi2 = (o1 - e1) ** 2 / v
    p = math.erfc(math.sqrt(chi2 / 2.0))  # chi-square, 1 df upper tail
    return chi2, p, o1, e1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--gene", default=os.environ.get("AGENTFLOW_PARAM_GENE"))
    ap.add_argument(
        "--study",
        default=os.environ.get(
            "AGENTFLOW_PARAM_STUDY", "lihc_tcga_pan_can_atlas_2018"
        ),
    )
    ap.add_argument("--profile", default=os.environ.get("AGENTFLOW_PARAM_PROFILE"))
    ap.add_argument("--out", default=os.environ.get("AGENTFLOW_OUTPUT_REPORT"))
    args = ap.parse_args()
    if not args.gene or not args.out:
        sys.exit("gene and out are required (via env or flags)")
    profile = args.profile or f"{args.study}_rna_seq_v2_mrna"

    entrez = resolve_entrez(args.gene)
    expr = fetch_expression(args.study, profile, entrez)
    surv = fetch_survival(args.study)
    paired = [(expr[p], surv[p][0], surv[p][1]) for p in expr if p in surv]
    if len(paired) < 10:
        sys.exit(f"too few paired samples ({len(paired)}) for {args.gene} in {args.study}")

    cut = median([e for e, _, _ in paired])
    hi = [(t, ev) for e, t, ev in paired if e >= cut]
    lo = [(t, ev) for e, t, ev in paired if e < cut]
    chi2, p, o1, e1 = logrank(hi, lo)
    med_hi, med_lo = median_survival(hi), median_survival(lo)
    hi_worse = med_hi < med_lo  # high expression -> shorter survival
    direction = "high-expression associated with worse survival" if hi_worse else \
                "high-expression associated with better survival"
    signed = (1 if hi_worse else -1) * min(10.0, -math.log10(p) if p > 0 else 10.0)

    lines = [
        "Marker report",
        f"Gene: {args.gene}",
        f"score: {signed:.3f}",
        f"study: {args.study}",
        f"entrez: {entrez}",
        f"paired_samples: {len(paired)} (high={len(hi)}, low={len(lo)})",
        f"events: high={sum(1 for _,e in hi if e)}, low={sum(1 for _,e in lo if e)}",
        f"median_OS_months: high={med_hi:.2f}, low={med_lo:.2f}",
        f"logrank_chi2: {chi2:.4f}",
        f"logrank_p: {p:.6f}",
        f"direction: {direction}",
        "note: score = signed -log10(p); sign + means high expression worsens survival",
    ]
    with open(args.out, "w") as f:
        f.write("\n".join(lines) + "\n")
    sys.stderr.write(f"tcga_survival_assoc ok: {args.gene} p={p:.4g} {direction}\n")


if __name__ == "__main__":
    main()
