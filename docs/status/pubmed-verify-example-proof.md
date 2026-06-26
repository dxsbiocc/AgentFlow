# 验证记录:PubMed E-utilities 核验脚本(examples/forage/pubmed_verify.py)

Date: 2026-06-26
Status: PASS — 真实联网 PubMed 核验器,用 NCBI E-utilities 按 PubMed 自带的 `Retracted Publication` 类型判定撤稿,产出带状态的 hits;AgentFlow ingest 时诚实分级。与 `crossref_verify.py` 互补(PubMed 是 PMID 原生、撤稿标记权威;Crossref 是 DOI 原生、解析预印本→发表)。核心仍离线。

## 信号(NCBI E-utilities esummary `result`)

- **撤稿**:记录 `pubtype` 含 `Retracted Publication`(PubMed 官方撤稿类型)。
- **id**:`articleids` 里的 DOI(优先),否则 `PMID:<uid>`。
- **访问级别**:`articleids` 含 PMC id(PubMed Central 免费全文)→ `open_access_full_text`,否则 abstract。

协议同 forage fetch:`--query --max --out`。`NCBI_EMAIL`/`NCBI_API_KEY` 做合规调用。`--self-test` 离线校验解析。

## 证据

- 离线 `--self-test`:三种真实 esummary 形态(撤稿+PMC / 开放+PMC / 闭源无 DOI)断言 `record_to_hit` 正确。
- **Live 真实查询**:`--query "cancer AND Retracted Publication[Publication Type]"` → 4 篇真实撤稿文章,全部 `retracted:true`,DOI 解析、PMC→OA。
- **端到端**:`forage fetch --query ... --script pubmed_verify.py --source pubmed` → ingest 3 条全标 `RETRACTED` → link 后 grade=`unsupported`。
- core 363(未改)+ acceptance 绿;`argument.rs` 空 diff;无新依赖(stdlib urllib)。

## 边界

- 面向已发表文献(PubMed 一般不索引预印本),故不设 `published_as`(发表升级用 crossref_verify)。
- 访问级别用 PMC 在场作启发式;精确 OA 可叠 Unpaywall(见 crossref_verify)。
- 网络在脚本里,受用户 egress 约束;核心不发请求。
