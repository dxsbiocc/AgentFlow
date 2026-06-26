#!/usr/bin/env python3
"""Example forage fetch/verify script: emit literature hits with *verified*
retraction / publication status.

`agentflow forage fetch --query <q> --script <this> [--source <s>]` (and
`agent run --auto-forage --forage-script <this>`) invoke it as:

    python verify_status.py --query <q> --max <n> --out <file>

The script writes JSON-Lines hits to `--out`, one object per line:

    {"external_id": "...", "title": "...", "access_status": "...",
     "retracted": <bool>, "published_as": "<published-id>"}

`access_status` is one of metadata_only | abstract_available |
open_access_full_text | user_provided_full_text |
subscription_connector_full_text | full_text_unavailable | retrieval_failed.
The optional `retracted` / `published_as` fields carry *verified* status — this
is where a real lookup belongs (PubMed EFetch, Crossref, Retraction Watch). They
are graded honestly by AgentFlow on ingest: a retracted source -> Unsupported, a
published preprint -> peer-reviewed grade, a bare preprint -> capped Hypothesis.

NETWORK: AgentFlow itself stays offline — only this user-controlled script
touches the network, under your egress policy. Replace `verify_status()` below
with a real query. The default implementation is an offline stub so the example
runs deterministically.
"""

import argparse
import json
import sys


def verify_status(external_id: str) -> dict:
    """Return {'retracted': bool, 'published_as': str|None} for an id.

    Replace this with a real lookup, e.g.:
      - retraction: query Retraction Watch / PubMed publication types for
        "Retracted Publication";
      - publication: resolve a preprint DOI to its published version via Crossref
        `relation.is-preprint-of` or the bioRxiv/medRxiv API `published` field.
    The offline stub below recognizes a couple of demo ids.
    """
    table = {
        "PMID:RETRACTED": {"retracted": True, "published_as": None},
        "doi:10.1101/published": {"retracted": False, "published_as": "PMID:40000000"},
    }
    return table.get(external_id, {"retracted": False, "published_as": None})


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--query", required=True)
    parser.add_argument("--max", type=int, default=10)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    # A real script turns --query into candidate ids via a search API. For the
    # example we emit a small deterministic set so it runs without network.
    candidates = [
        ("PMID:RETRACTED", "Retracted study on " + args.query, "open_access_full_text"),
        ("doi:10.1101/published", "Preprint (since published) on " + args.query, "user_provided_full_text"),
        ("doi:10.1101/preprint", "Preprint on " + args.query, "user_provided_full_text"),
    ][: max(args.max, 0)]

    with open(args.out, "w") as handle:
        for external_id, title, access_status in candidates:
            status = verify_status(external_id)
            hit = {
                "external_id": external_id,
                "title": title,
                "access_status": access_status,
                "retracted": status["retracted"],
            }
            if status["published_as"]:
                hit["published_as"] = status["published_as"]
            handle.write(json.dumps(hit) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
