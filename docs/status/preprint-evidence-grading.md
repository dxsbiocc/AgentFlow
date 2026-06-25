# 验证记录:证据广度 — 预印本来源诚实分级(literature 超越 PubMed 元数据)

Date: 2026-06-25
Status: PASS — forage 现在能把 bioRxiv/medRxiv 等**预印本**作为一等证据来源,但其全文被**诚实地低于同行评议**分级,不会拔高置信度。

## 动机

forage 子系统本就**来源无关**(`forage observe --source <任意>`)且建模了全文访问级别。但 `link_forage_evidence` 用 `grade_from_access` 对**任何**全文都给 `LiteratureSupported`——于是一篇 bioRxiv 预印本全文和一篇同行评议 PMC 论文**同级**。这不诚实:预印本未经同行评议,证据强度应被压低。要"超越 PubMed 元数据"地扩展证据广度,前提是先把不同来源类型**诚实地分级**,否则引入预印本反而会过度信任。

## 改动(`crates/agentflow-core/src/forage.rs`)

- `is_preprint_source(source_id)`:大小写无关地识别已知预印本服务器(biorxiv/medrxiv/arxiv/chemrxiv/techrxiv/researchsquare/ssrn/preprints.org/osf/zenodo…),按子串匹配(`bioRxiv:2024.01` 也命中)。
- `grade_for_forage_source(access, source_id)`:在 `grade_from_access` 基础上,**若来源是预印本且 access 本会给 `LiteratureSupported`,则封顶为 `Hypothesis`**;非预印本来源分级不变。
- `link_forage_evidence` 改走 `grade_for_forage_source`(原来只看 access)。

诚实联动:`Hypothesis` 级证据**不能让判决 affirm**(grade-cap),所以仅靠预印本全文,verdict 无法被肯定——与整套诚实不变量一致。

## 证据

- 单测 `preprint_full_text_is_capped_below_peer_reviewed`:bioRxiv/medRxiv/arXiv/SSRN/preprints.org 的全文(open/user-provided)→ `Hypothesis`;pubmed/pmc/doi/cbioportal 的全文 → `LiteratureSupported`。
- 修正了既有测试 `links_forage_observation_into_evidence_ledger`:它本就用 `"biorxiv"` + 全文却断言 `LiteratureSupported`(标题写着 "Full-text preprint")——正是本次要修的过度信任,改断言为 `Hypothesis`。
- **Live**(agent CLI):同一假设下 forage 一篇 bioRxiv 全文 + 一篇 PubMed 全文并 link —— `evidence list` 显示
  - `source=doi:10.1101/2026.01.01  grade=hypothesis`(预印本封顶)
  - `source=PMID:99999            grade=literature_supported`(同行评议)
- core 358(+1)+ cli + acceptance 绿;clippy 干净;`argument.rs` 空 diff;无新依赖。

## 边界(诚实)

- 预印本全文统一封顶到 `Hypothesis`(枚举里没有"预印本支持"这一档);它仍能支撑/生成假设,只是不算同行评议支持。
- 未知来源默认按 access 分级(不强行下调),避免误伤 PMC/DOI 等同行评议全文;识别表是已知预印本服务器的白名单,可扩充。
- 这是"证据广度"的**使能半步**(让预印本可被诚实使用);自动抓取预印本全文、区分"已撤回/已发表升级"等是后续。
