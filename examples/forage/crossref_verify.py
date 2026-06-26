#!/usr/bin/env python3
"""Real Crossref-backed forage fetch/verify script.

Searches the public Crossref REST API and emits literature hits whose retraction
and publication status are *verified* from Crossref metadata, so AgentFlow grades
them honestly on ingest (a retracted source -> Unsupported, a published preprint
-> peer-reviewed grade, a bare preprint -> capped Hypothesis). This is the real
counterpart to the offline template `verify_status.py`.

Protocol (invoked by `agentflow forage fetch --script` and
`agent run --auto-forage --forage-script`):

    python crossref_verify.py --query <q> --max <n> --out <file>

It writes JSON-Lines hits to `--out`:

    {"external_id": "doi:...", "title": "...", "access_status": "...",
     "retracted": <bool?>, "published_as": "doi:..."?}

Signals used (all from Crossref `message`):
  - retraction: a `RETRACTED` title prefix, or an `update-to` entry of
    `type: retraction`;
  - publication of a preprint: `relation.is-preprint-of[].id` (the published DOI);
  - access: preprints (`type: posted-content`) and items carrying a license are
    treated as open-access full text, otherwise abstract-only.

NETWORK: AgentFlow stays offline — only this user-controlled script calls the
network, under your egress policy. Set CROSSREF_MAILTO to use Crossref's polite
pool. Run `python crossref_verify.py --self-test` to validate the parsing offline
without any network call.

Guidance: pass `forage fetch --source biorxiv` (or the server you searched) so a
bare preprint is correctly preprint-graded; a published preprint lifts the cap
via `published_as`, and a retraction dominates regardless of source.
"""

import argparse
import json
import os
import sys
import urllib.parse
import urllib.request

CROSSREF_WORKS = "https://api.crossref.org/works"


def is_retracted(work: dict) -> bool:
    title = (work.get("title") or [""])[0]
    if title.strip().upper().startswith("RETRACTED"):
        return True
    return any(u.get("type") == "retraction" for u in work.get("update-to", []) or [])


def published_doi(work: dict) -> "str | None":
    if work.get("type") == "posted-content":
        for relation in work.get("relation", {}).get("is-preprint-of", []) or []:
            doi = relation.get("id")
            if doi:
                return doi
    return None


def access_status(work: dict) -> str:
    # A preprint's full text is openly readable; a licensed item is treated as OA.
    if work.get("type") == "posted-content" or work.get("license"):
        return "open_access_full_text"
    return "abstract_available"


def to_hit(work: dict) -> "dict | None":
    doi = work.get("DOI")
    if not doi:
        return None
    title = (work.get("title") or ["(untitled)"])[0]
    hit = {
        "external_id": "doi:" + doi,
        "title": title,
        "access_status": access_status(work),
    }
    if is_retracted(work):
        hit["retracted"] = True
    published = published_doi(work)
    if published:
        hit["published_as"] = "doi:" + published
    return hit


def search(query: str, rows: int) -> list:
    params = {"query.bibliographic": query, "rows": str(max(rows, 0))}
    mailto = os.environ.get("CROSSREF_MAILTO", "").strip()
    if mailto:
        params["mailto"] = mailto
    url = CROSSREF_WORKS + "?" + urllib.parse.urlencode(params)
    request = urllib.request.Request(
        url, headers={"User-Agent": "AgentFlow-forage-verify/0.1 (https://github.com/dxsbiocc/AgentFlow)"}
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        return json.load(response)["message"]["items"]


def self_test() -> int:
    # Fixtures mirror real Crossref `message` shapes (see module docstring).
    published_preprint = {
        "DOI": "10.1101/2020.08.12.20173690",
        "type": "posted-content",
        "title": ["A preprint that was later published"],
        "relation": {"is-preprint-of": [{"id": "10.1038/s41467-021-21237-w"}]},
    }
    bare_preprint = {
        "DOI": "10.1101/2026.01.01.000000",
        "type": "posted-content",
        "title": ["A preprint with no published version"],
    }
    retracted = {
        "DOI": "10.1007/s00132-015-3148-2",
        "type": "journal-article",
        "title": ["RETRACTED ARTICLE: Osteoonkologie"],
        "update-to": [{"type": "retraction", "DOI": "10.1007/s00132-015-3148-2"}],
    }
    a = to_hit(published_preprint)
    assert a["published_as"] == "doi:10.1038/s41467-021-21237-w", a
    assert a["access_status"] == "open_access_full_text" and "retracted" not in a, a
    b = to_hit(bare_preprint)
    assert "published_as" not in b and "retracted" not in b, b
    assert b["access_status"] == "open_access_full_text", b
    c = to_hit(retracted)
    assert c["retracted"] is True and "published_as" not in c, c
    print("crossref_verify self-test: ok")
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
        sys.stderr.write("crossref_verify requires --query and --out (or --self-test)\n")
        return 2

    try:
        items = search(args.query, args.max)
    except Exception as error:  # noqa: BLE001 - report any fetch failure to the caller
        sys.stderr.write(f"crossref query failed: {error}\n")
        return 1

    written = 0
    with open(args.out, "w") as handle:
        for work in items:
            hit = to_hit(work)
            if hit:
                handle.write(json.dumps(hit) + "\n")
                written += 1
    sys.stderr.write(f"crossref_verify ok: {written} hits for {args.query!r}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
