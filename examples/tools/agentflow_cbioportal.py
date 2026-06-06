#!/usr/bin/env python3
"""Verified cBioPortal client helpers for AgentFlow synthesized tools.

This module is intentionally small and stdlib-only. It exposes real cBioPortal
access patterns extracted from examples/tools/tcga_survival_assoc.py so LLM
synthesized tools can import a known-good client and focus on analysis logic.

Functions raise CBioPortalError when the requested real data cannot be fetched or
parsed. They never fabricate fallback data.

Example:
    import agentflow_cbioportal as cbio

    gene = "MID1IP1"
    study = cbio.resolve_study("hepatocellular carcinoma")
    expression_by_sample = cbio.fetch_expression(study, gene)
    survival_by_patient = cbio.fetch_overall_survival(study)
"""

from __future__ import annotations

import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

API = os.environ.get("AGENTFLOW_CBIOPORTAL_API_BASE", "https://www.cbioportal.org/api")
GET_TIMEOUT = 60
POST_TIMEOUT = 90


class CBioPortalError(RuntimeError):
    """Raised when cBioPortal returns no usable real data for the request."""


def _api_url(path: str) -> str:
    if path.startswith("http://") or path.startswith("https://"):
        return path
    return API.rstrip("/") + "/" + path.lstrip("/")


def _get(url: str, timeout: float = GET_TIMEOUT):
    request = urllib.request.Request(_api_url(url), headers={"Accept": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return json.load(response)
    except (urllib.error.URLError, urllib.error.HTTPError, json.JSONDecodeError) as error:
        raise CBioPortalError(f"cBioPortal GET failed for {_api_url(url)}: {error}") from error


def _post(url: str, body: dict, timeout: float = POST_TIMEOUT):
    request = urllib.request.Request(
        _api_url(url),
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json", "Accept": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return json.load(response)
    except (urllib.error.URLError, urllib.error.HTTPError, json.JSONDecodeError) as error:
        raise CBioPortalError(f"cBioPortal POST failed for {_api_url(url)}: {error}") from error


def resolve_study(cancer_keyword: str) -> str:
    """Return a real cBioPortal study_id for a cancer keyword.

    PanCancer Atlas TCGA studies are preferred when multiple studies match. Raises
    CBioPortalError if no study can be resolved.
    """

    terms = _cancer_terms(cancer_keyword)
    if not terms:
        raise CBioPortalError("cancer_keyword is required to resolve a cBioPortal study")

    studies = _expect_list(_get("/studies"), "studies")
    scored = []
    for study in studies:
        study_id = str(study.get("studyId") or "")
        score = _score_study(study, terms)
        if study_id and score is not None:
            scored.append((score, study_id))

    if not scored:
        raise CBioPortalError(f"no cBioPortal study matched {cancer_keyword!r}")

    scored.sort(reverse=True)
    return scored[0][1]


def fetch_expression(study_id: str, gene: str) -> dict[str, float]:
    """Return sample_id -> mRNA expression value for a gene in a study.

    The function resolves the study's mRNA molecular profile and all-samples list,
    then uses cBioPortal's molecular-data POST fetch endpoint. Raises CBioPortalError
    when the gene, profile, sample list, or expression rows are unavailable.
    """

    study_id = _require_text(study_id, "study_id")
    gene = _require_text(gene, "gene")
    profile_id = _resolve_mrna_profile(study_id)
    sample_list_id = _resolve_sample_list(study_id)
    entrez = resolve_entrez(gene)

    encoded_profile = urllib.parse.quote(profile_id, safe="")
    rows = _expect_list(
        _post(
            f"/molecular-profiles/{encoded_profile}/molecular-data/fetch?projection=SUMMARY",
            {"entrezGeneIds": [entrez], "sampleListId": sample_list_id},
        ),
        "molecular data",
    )
    values = {}
    for row in rows:
        sample_id = row.get("sampleId")
        value = row.get("value")
        if not sample_id or value is None:
            continue
        try:
            values[str(sample_id)] = float(value)
        except (TypeError, ValueError):
            continue

    if not values:
        raise CBioPortalError(f"no expression values for gene {gene!r} in study {study_id!r}")
    return values


def fetch_overall_survival(study_id: str) -> dict[str, tuple[float, bool]]:
    """Return patient_id -> (OS_MONTHS, OS event bool) for a study.

    OS event is True for deceased/event and False for censored/living. Clinical
    data is fetched through cBioPortal's clinical-data POST fetch endpoint.
    Raises CBioPortalError when usable OS data is unavailable.
    """

    data = _fetch_clinical_attributes(study_id, ["OS_MONTHS", "OS_STATUS"])
    months = data.get("OS_MONTHS", {})
    statuses = data.get("OS_STATUS", {})
    survival = {}
    for patient_id, raw_months in months.items():
        if patient_id not in statuses:
            continue
        try:
            parsed_months = float(raw_months)
        except (TypeError, ValueError):
            continue
        event = _parse_os_event(statuses[patient_id])
        if parsed_months >= 0 and event is not None:
            survival[patient_id] = (parsed_months, event)

    if not survival:
        raise CBioPortalError(f"no overall survival data for study {study_id!r}")
    return survival


def fetch_clinical_attribute(study_id: str, attr: str) -> dict[str, str]:
    """Return patient_id -> clinical attribute value using clinical-data POST fetch."""

    attr = _require_text(attr, "attr")
    data = _fetch_clinical_attributes(study_id, [attr]).get(attr, {})
    if not data:
        raise CBioPortalError(f"clinical attribute {attr!r} has no data in study {study_id!r}")
    return data


def resolve_entrez(gene: str) -> int:
    """Return the Entrez gene id for a gene symbol or numeric Entrez id."""

    gene = _require_text(gene, "gene")
    if gene.isdigit():
        return int(gene)
    info = _get(f"/genes/{urllib.parse.quote(gene, safe='')}")
    try:
        return int(info["entrezGeneId"])
    except (KeyError, TypeError, ValueError) as error:
        raise CBioPortalError(f"could not resolve Entrez id for gene {gene!r}") from error


def _fetch_clinical_attributes(study_id: str, attrs: list[str]) -> dict[str, dict[str, str]]:
    study_id = _require_text(study_id, "study_id")
    attrs = [_require_text(attr, "attr") for attr in attrs]
    encoded_study = urllib.parse.quote(study_id, safe="")
    by_attr = {attr: {} for attr in attrs}

    # cBioPortal's clinical-data/fetch POST endpoint rejects this shape (HTTP 400);
    # the GET clinical-data endpoint returns all patient rows and is filtered here
    # (matches the verified tcga_survival_assoc.py access pattern).
    rows = _expect_list(
        _get(
            f"/studies/{encoded_study}/clinical-data"
            "?clinicalDataType=PATIENT&projection=SUMMARY&pageSize=100000"
        ),
        "clinical data",
    )
    for row in rows:
        attr = row.get("clinicalAttributeId")
        patient_id = row.get("patientId")
        value = row.get("value")
        if attr in by_attr and patient_id and value not in (None, ""):
            by_attr[attr][str(patient_id)] = str(value)

    if not any(by_attr.values()):
        raise CBioPortalError(
            f"no clinical data for attributes {', '.join(attrs)} in study {study_id!r}"
        )
    return by_attr


def _fetch_patient_ids(study_id: str) -> list[str]:
    encoded_study = urllib.parse.quote(study_id, safe="")
    rows = _expect_list(
        _get(f"/studies/{encoded_study}/patients?projection=SUMMARY&pageSize=100000"),
        "patients",
    )
    patient_ids = [str(row.get("patientId")) for row in rows if row.get("patientId")]
    if not patient_ids:
        raise CBioPortalError(f"no patients found for study {study_id!r}")
    return patient_ids


def _resolve_mrna_profile(study_id: str) -> str:
    encoded_study = urllib.parse.quote(study_id, safe="")
    profiles = _expect_list(
        _get(f"/studies/{encoded_study}/molecular-profiles"), "molecular profiles"
    )
    scored = []
    for profile in profiles:
        profile_id = str(profile.get("molecularProfileId") or "")
        score = _score_mrna_profile(profile)
        if profile_id and score is not None:
            scored.append((score, profile_id))
    if not scored:
        raise CBioPortalError(f"no mRNA expression profile found for study {study_id!r}")
    scored.sort(reverse=True)
    return scored[0][1]


def _resolve_sample_list(study_id: str) -> str:
    encoded_study = urllib.parse.quote(study_id, safe="")
    sample_lists = _expect_list(_get(f"/studies/{encoded_study}/sample-lists"), "sample lists")
    scored = []
    for sample_list in sample_lists:
        sample_list_id = str(sample_list.get("sampleListId") or "")
        score = _score_sample_list(sample_list, study_id)
        if sample_list_id and score is not None:
            scored.append((score, sample_list_id))
    if not scored:
        raise CBioPortalError(f"no usable sample list found for study {study_id!r}")
    scored.sort(reverse=True)
    return scored[0][1]


def _cancer_terms(keyword: str) -> list[str]:
    lower = _require_text(keyword, "cancer_keyword").lower()
    groups = [
        (["liver", "hepatocellular", "hcc", "lihc"], ["lihc", "hepatocellular", "liver", "hcc"]),
        (["breast", "brca"], ["brca", "breast"]),
        (["lung", "luad", "lusc", "nsclc"], ["luad", "lusc", "lung", "nsclc"]),
        (["colon", "colorectal", "coad", "read"], ["coad", "read", "colorectal", "colon"]),
        (["prostate", "prad"], ["prad", "prostate"]),
        (["ovarian", "ovary", "ov"], ["ov", "ovarian"]),
        (["melanoma", "skcm"], ["skcm", "melanoma"]),
        (["pancreatic", "pancreas", "paad"], ["paad", "pancreatic"]),
        (["glioblastoma", "gbm"], ["gbm", "glioblastoma"]),
        (["kidney", "renal", "kirc", "kirp"], ["kirc", "kirp", "kidney", "renal"]),
        (["gastric", "stomach", "stad"], ["stad", "gastric", "stomach"]),
        (["bladder", "blca"], ["blca", "bladder"]),
        (["endometrial", "uterine", "ucec"], ["ucec", "endometrial", "uterine"]),
        (["head and neck", "hnsc"], ["hnsc", "head and neck"]),
        (["thyroid", "thca"], ["thca", "thyroid"]),
        (["leukemia", "aml", "laml"], ["laml", "aml", "leukemia"]),
    ]
    terms = []
    for needles, group_terms in groups:
        if any(_contains_term(lower, needle) for needle in needles):
            terms.extend(term for term in group_terms if term not in terms)
    for token in lower.replace("-", " ").replace("_", " ").split():
        if len(token) > 2 and token not in terms:
            terms.append(token)
    return terms


def _score_study(study: dict, terms: list[str]) -> int | None:
    study_id = str(study.get("studyId") or "").lower()
    cancer_type = str(study.get("cancerTypeId") or "").lower()
    name = str(study.get("name") or "").lower()
    description = str(study.get("description") or "").lower()
    haystack = f"{study_id} {cancer_type} {name} {description}"
    score = 0
    matched = False
    for term in terms:
        if _contains_term(study_id, term) or _contains_term(cancer_type, term):
            score += 45
            matched = True
        elif _contains_term(haystack, term):
            score += 20
            matched = True
    if not matched:
        return None
    if "pan_can_atlas" in study_id or "pancancer atlas" in name:
        score += 80
    if "tcga" in study_id:
        score += 25
    if "2018" in study_id:
        score += 5
    if "cell_line" in study_id or "ccle" in study_id:
        score -= 30
    return score


def _score_mrna_profile(profile: dict) -> int | None:
    profile_id = str(profile.get("molecularProfileId") or "").lower()
    alteration_type = str(profile.get("molecularAlterationType") or "").lower()
    datatype = str(profile.get("datatype") or "").lower()
    name = str(profile.get("name") or "").lower()
    description = str(profile.get("description") or "").lower()
    haystack = f"{profile_id} {alteration_type} {datatype} {name} {description}"
    score = 0
    if alteration_type == "mrna_expression":
        score += 100
    if "mrna" in haystack:
        score += 35
    if "rna_seq" in haystack or "rna seq" in haystack:
        score += 30
    if "expression" in haystack:
        score += 20
    if datatype == "continuous":
        score += 20
    if "rna_seq_v2_mrna" in profile_id:
        score += 70
    if profile_id.endswith("_mrna"):
        score += 20
    if "zscore" in haystack or "z-score" in haystack:
        score -= 25
    return score if score > 0 else None


def _score_sample_list(sample_list: dict, study_id: str) -> int | None:
    sample_list_id = str(sample_list.get("sampleListId") or "").lower()
    category = str(sample_list.get("category") or "").lower()
    name = str(sample_list.get("name") or "").lower()
    description = str(sample_list.get("description") or "").lower()
    haystack = f"{sample_list_id} {category} {name} {description}"
    score = 0
    if sample_list_id == f"{study_id.lower()}_all":
        score += 120
    if sample_list_id.endswith("_all"):
        score += 80
    if "all_cases" in category or "all samples" in haystack:
        score += 45
    if "sequenced" in sample_list_id:
        score += 15
    return score if score > 0 else None


def _parse_os_event(value) -> bool | None:
    text = str(value).strip().lower()
    if text.startswith("1") or "deceased" in text or "dead" in text:
        return True
    if text.startswith("0") or "living" in text or "alive" in text:
        return False
    return None


def _contains_term(haystack: str, needle: str) -> bool:
    if needle.isalnum() and len(needle) <= 4:
        start = 0
        while True:
            index = haystack.find(needle, start)
            if index < 0:
                return False
            end = index + len(needle)
            before = haystack[index - 1] if index else ""
            after = haystack[end] if end < len(haystack) else ""
            if (not before.isalnum()) and (not after.isalnum()):
                return True
            start = end
    return needle in haystack


def _chunks(values: list[str], size: int):
    for index in range(0, len(values), size):
        yield values[index : index + size]


def _expect_list(value, label: str) -> list:
    if not isinstance(value, list):
        raise CBioPortalError(f"expected {label} response to be a list")
    return value


def _require_text(value: str, name: str) -> str:
    text = str(value or "").strip()
    if not text:
        raise CBioPortalError(f"{name} is required")
    return text


def _self_test() -> None:
    original_get = globals()["_get"]
    original_post = globals()["_post"]
    post_calls = []

    def fake_get(url, timeout=GET_TIMEOUT):
        if url == "/studies":
            return [
                {
                    "studyId": "lihc_tcga",
                    "name": "Liver Hepatocellular Carcinoma",
                    "description": "Legacy LIHC",
                    "cancerTypeId": "lihc",
                },
                {
                    "studyId": "lihc_tcga_pan_can_atlas_2018",
                    "name": "Liver Hepatocellular Carcinoma (TCGA, PanCancer Atlas)",
                    "description": "Hepatocellular carcinoma",
                    "cancerTypeId": "lihc",
                },
            ]
        if url == "/studies/lihc_tcga_pan_can_atlas_2018/molecular-profiles":
            return [
                {
                    "molecularProfileId": "lihc_tcga_pan_can_atlas_2018_mutations",
                    "molecularAlterationType": "MUTATION_EXTENDED",
                    "datatype": "MAF",
                    "name": "Mutations",
                },
                {
                    "molecularProfileId": "lihc_tcga_pan_can_atlas_2018_rna_seq_v2_mrna",
                    "molecularAlterationType": "MRNA_EXPRESSION",
                    "datatype": "CONTINUOUS",
                    "name": "mRNA expression (RNA Seq V2 RSEM)",
                },
            ]
        if url == "/studies/lihc_tcga_pan_can_atlas_2018/sample-lists":
            return [
                {
                    "sampleListId": "lihc_tcga_pan_can_atlas_2018_sequenced",
                    "category": "all_cases_with_mutation_and_cna_data",
                    "name": "Sequenced tumors",
                },
                {
                    "sampleListId": "lihc_tcga_pan_can_atlas_2018_all",
                    "category": "all_cases_in_study",
                    "name": "All samples",
                },
            ]
        if url == "/genes/TP53":
            return {"entrezGeneId": 7157}
        if url == "/studies/lihc_tcga_pan_can_atlas_2018/patients?projection=SUMMARY&pageSize=100000":
            return [{"patientId": "P1"}, {"patientId": "P2"}]
        if url == (
            "/studies/lihc_tcga_pan_can_atlas_2018/clinical-data"
            "?clinicalDataType=PATIENT&projection=SUMMARY&pageSize=100000"
        ):
            return [
                {"patientId": "P1", "clinicalAttributeId": "OS_MONTHS", "value": "12.5"},
                {"patientId": "P2", "clinicalAttributeId": "OS_MONTHS", "value": "31"},
                {"patientId": "P1", "clinicalAttributeId": "OS_STATUS", "value": "1:DECEASED"},
                {"patientId": "P2", "clinicalAttributeId": "OS_STATUS", "value": "0:LIVING"},
            ]
        raise AssertionError(f"unexpected GET {url}")

    def fake_post(url, body, timeout=POST_TIMEOUT):
        post_calls.append((url, body))
        if "/molecular-data/fetch" in url:
            assert body == {
                "entrezGeneIds": [7157],
                "sampleListId": "lihc_tcga_pan_can_atlas_2018_all",
            }
            return [
                {"sampleId": "S1", "patientId": "P1", "value": 2.5},
                {"sampleId": "S2", "patientId": "P2", "value": "4.0"},
            ]
        if "/clinical-data/fetch" in url:
            assert body["clinicalDataType"] == "PATIENT"
            assert body["ids"] == ["P1", "P2"]
            attrs = set(body["attributeIds"])
            rows = []
            if "OS_MONTHS" in attrs:
                rows.extend(
                    [
                        {"patientId": "P1", "clinicalAttributeId": "OS_MONTHS", "value": "12.5"},
                        {"patientId": "P2", "clinicalAttributeId": "OS_MONTHS", "value": "31"},
                    ]
                )
            if "OS_STATUS" in attrs:
                rows.extend(
                    [
                        {"patientId": "P1", "clinicalAttributeId": "OS_STATUS", "value": "1:DECEASED"},
                        {"patientId": "P2", "clinicalAttributeId": "OS_STATUS", "value": "0:LIVING"},
                    ]
                )
            return rows
        raise AssertionError(f"unexpected POST {url}")

    try:
        globals()["_get"] = fake_get
        globals()["_post"] = fake_post
        study = resolve_study("hepatocellular carcinoma")
        assert study == "lihc_tcga_pan_can_atlas_2018"
        expression = fetch_expression(study, "TP53")
        assert expression == {"S1": 2.5, "S2": 4.0}
        survival = fetch_overall_survival(study)
        assert survival == {"P1": (12.5, True), "P2": (31.0, False)}
        assert any("/molecular-data/fetch" in url for url, _ in post_calls)
        # clinical data is fetched via GET /clinical-data (see _fetch_clinical_attributes);
        # the survival assertion above proves that path ran.
    finally:
        globals()["_get"] = original_get
        globals()["_post"] = original_post


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv == ["--self-test"]:
        _self_test()
        print("agentflow_cbioportal self-test ok")
        return 0
    print(__doc__.strip())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
