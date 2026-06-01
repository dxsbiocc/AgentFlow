#!/usr/bin/env python3
# Usage: python3 examples/tools/pubmed_search.py --query "marker pathway" --max 10 --out hits.jsonl

import argparse
import json
import urllib.parse
import urllib.request


ESEARCH_URL = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi"
ESUMMARY_URL = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esummary.fcgi"


def fetch_json(url, params):
    query = urllib.parse.urlencode(params)
    with urllib.request.urlopen(f"{url}?{query}", timeout=30) as response:
        return json.loads(response.read().decode("utf-8"))


def search_pubmed(query, max_hits):
    search = fetch_json(
        ESEARCH_URL,
        {
            "db": "pubmed",
            "term": query,
            "retmode": "json",
            "retmax": str(max_hits),
        },
    )
    id_list = search.get("esearchresult", {}).get("idlist", [])
    if not id_list:
        return []

    summary = fetch_json(
        ESUMMARY_URL,
        {
            "db": "pubmed",
            "id": ",".join(id_list),
            "retmode": "json",
        },
    )
    result = summary.get("result", {})
    hits = []
    for pmid in id_list:
        title = result.get(pmid, {}).get("title")
        if not isinstance(title, str) or not title.strip():
            continue
        hits.append(
            {
                "external_id": f"PMID:{pmid}",
                "title": title.strip(),
                "access_status": "abstract_available",
            }
        )
    return hits


def write_jsonl(path, hits):
    with open(path, "w", encoding="utf-8") as output:
        for hit in hits:
            output.write(json.dumps(hit, ensure_ascii=False, separators=(",", ":")))
            output.write("\n")


def main():
    parser = argparse.ArgumentParser(description="Search PubMed and write AgentFlow forage JSONL.")
    parser.add_argument("--query", required=True)
    parser.add_argument("--max", required=True, type=int, dest="max_hits")
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    if args.max_hits < 0:
        raise SystemExit("--max must be a non-negative integer")

    try:
        write_jsonl(args.out, search_pubmed(args.query, args.max_hits))
    except Exception as error:
        try:
            write_jsonl(args.out, [])
        except Exception:
            pass
        raise SystemExit(f"pubmed_search failed: {error}")


if __name__ == "__main__":
    main()
