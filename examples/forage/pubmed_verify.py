#!/usr/bin/env python3
"""NCBI PubMed (E-utilities) forage fetch/verify script.

Searches PubMed via E-utilities and emits literature hits whose retraction status
is verified from PubMed's own metadata, so AgentFlow grades them honestly on
ingest (a retracted source -> Unsupported). Complements `crossref_verify.py`:
PubMed flags retractions with the `Retracted Publication` publication type and is
PMID-native, while Crossref is DOI-native and resolves preprint->published
relations. Use whichever indexes your sources (or both).

Protocol (invoked by `agentflow forage fetch --script` and
`agent run --auto-forage --forage-script`):

    python pubmed_verify.py --query <q> --max <n> --out <file>

It writes JSON-Lines hits to `--out`:

    {"external_id": "doi:..."|"PMID:...", "title": "...", "access_status": "...",
     "retracted": <bool?>}

Signals (from E-utilities esummary `result`):
  - retraction: `Retracted Publication` in the record's `pubtype`;
  - id: the record's DOI from `articleids` if present, else `PMID:<uid>`;
  - access: a PMC id in `articleids` (free full text in PubMed Central) -> open
    access full text, otherwise abstract-only.

NETWORK: AgentFlow stays offline — only this user-controlled script calls the
network, under your egress policy. Set NCBI_EMAIL (and optionally NCBI_API_KEY)
to be a good E-utilities citizen. Run `python pubmed_verify.py --self-test` to
validate the parsing offline without any network call.

Guidance: pass `forage fetch --source pubmed` to label the batch.
"""

import argparse
import json
import os
import sys
import urllib.parse
import urllib.request

EUTILS = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/"


def record_to_hit(record: dict, pmid: str) -> dict:
    article_ids = record.get("articleids", []) or []
    doi = next(
        (a.get("value") for a in article_ids if a.get("idtype") == "doi" and a.get("value")),
        None,
    )
    has_pmc = any(a.get("idtype") in ("pmc", "pmcid") for a in article_ids)
    title = (record.get("title") or "").strip().rstrip(".") or "(untitled)"
    hit = {
        "external_id": "doi:" + doi if doi else "PMID:" + str(pmid),
        "title": title,
        "access_status": "open_access_full_text" if has_pmc else "abstract_available",
    }
    if "Retracted Publication" in (record.get("pubtype") or []):
        hit["retracted"] = True
    return hit


def _eutils_params(extra: dict) -> dict:
    params = dict(extra)
    email = os.environ.get("NCBI_EMAIL", "").strip()
    if email:
        params["email"] = email
    api_key = os.environ.get("NCBI_API_KEY", "").strip()
    if api_key:
        params["api_key"] = api_key
    return params


def _get(endpoint: str, params: dict) -> dict:
    url = EUTILS + endpoint + "?" + urllib.parse.urlencode(params)
    request = urllib.request.Request(
        url, headers={"User-Agent": "AgentFlow-forage-verify/0.1 (https://github.com/dxsbiocc/AgentFlow)"}
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        return json.load(response)


def esearch(query: str, retmax: int) -> list:
    data = _get(
        "esearch.fcgi",
        _eutils_params({"db": "pubmed", "term": query, "retmax": str(max(retmax, 0)), "retmode": "json"}),
    )
    return data.get("esearchresult", {}).get("idlist", [])


ESUMMARY_BATCH = 200


def esummary(pmids: list) -> dict:
    # Batch ids (NCBI recommends <= 200 per esummary call) to stay under URL limits.
    result = {}
    for start in range(0, len(pmids), ESUMMARY_BATCH):
        batch = pmids[start : start + ESUMMARY_BATCH]
        data = _get(
            "esummary.fcgi",
            _eutils_params({"db": "pubmed", "id": ",".join(batch), "retmode": "json"}),
        )
        result.update(data.get("result", {}))
    return result


def self_test() -> int:
    retracted = {
        "title": "RETRACTED: NFIC suppressed glioma.",
        "pubtype": ["Journal Article", "Retracted Publication"],
        "articleids": [
            {"idtype": "pmc", "value": "PMC12978455"},
            {"idtype": "doi", "value": "10.1371/journal.pone.0341816"},
        ],
    }
    open_paper = {
        "title": "An open-access study.",
        "pubtype": ["Journal Article"],
        "articleids": [{"idtype": "pmc", "value": "PMC1"}, {"idtype": "doi", "value": "10.1/x"}],
    }
    closed_no_doi = {
        "title": "A closed paper",
        "pubtype": ["Journal Article"],
        "articleids": [{"idtype": "pubmed", "value": "99"}],
    }
    a = record_to_hit(retracted, "1")
    assert a["external_id"] == "doi:10.1371/journal.pone.0341816", a
    assert a["title"] == "RETRACTED: NFIC suppressed glioma", a
    assert a["retracted"] is True and a["access_status"] == "open_access_full_text", a
    b = record_to_hit(open_paper, "2")
    assert "retracted" not in b and b["access_status"] == "open_access_full_text", b
    c = record_to_hit(closed_no_doi, "99")
    assert c["external_id"] == "PMID:99" and c["access_status"] == "abstract_available", c
    assert "retracted" not in c, c
    print("pubmed_verify self-test: ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--query")
    parser.add_argument("--max", type=int, default=10)
    parser.add_argument("--out")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        return self_test()
    if not args.query or not args.out:
        sys.stderr.write("pubmed_verify requires --query and --out (or --self-test)\n")
        return 2

    try:
        pmids = esearch(args.query, args.max)
        records = esummary(pmids)
    except Exception as error:  # noqa: BLE001 - report any fetch failure to the caller
        sys.stderr.write(f"pubmed query failed: {error}\n")
        return 1

    written = 0
    with open(args.out, "w") as handle:
        for pmid in pmids:
            record = records.get(pmid)
            if not isinstance(record, dict):
                continue
            hit = record_to_hit(record, pmid)
            if hit:
                handle.write(json.dumps(hit) + "\n")
                written += 1
    sys.stderr.write(f"pubmed_verify ok: {written} hits for {args.query!r}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
