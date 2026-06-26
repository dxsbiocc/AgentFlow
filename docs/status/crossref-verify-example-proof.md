# 验证记录:真·Crossref 文献状态核验脚本(examples/forage/crossref_verify.py)

Date: 2026-06-26
Status: PASS — #103 的核验模板配上了**真实联网实现**:查 Crossref 公共 API,自动判定预印本是否已发表、文章是否已撤稿,产出带 `retracted`/`published_as` 的 hits;AgentFlow ingest 时诚实分级。核心仍离线,网络只在脚本里。

## 背景

#103 让 forage hits JSONL 可携带核验状态,并给了离线模板 `verify_status.py`。本次补上**真实**实现 `crossref_verify.py`,把"自动核验"从模板变成可用工具。纯 `examples/` + docs,无核心改动(`argument.rs`/core 不动)。

## 信号(全部来自 Crossref `message`)

- **撤稿**:标题以 `RETRACTED` 开头,或 `update-to` 含 `type: retraction`。
- **预印本→已发表**:`relation.is-preprint-of[].id`(已发表版 DOI)。
- **访问级别**:预印本(`type: posted-content`)或带 license 的条目按 `open_access_full_text`,否则 `abstract_available`。

协议同 forage fetch:`python crossref_verify.py --query <q> --max <n> --out <file>`。`CROSSREF_MAILTO` 走 polite pool。`--self-test` 离线校验解析逻辑(无网络)。

## 证据

- **离线 self-test**:对三种真实 Crossref 形态(已发表预印本 / 裸预印本 / 撤稿文章)断言 `to_hit` 产出正确——`python crossref_verify.py --self-test` → ok。
- **Live 真实查询**:`--query "SPP1 hepatocellular carcinoma survival"` → 5 条真实 hit,access 正确判定。
- **真实已发表预印本**:`--query "single-cell RNA sequencing atlas"` → `doi:10.1101/826560 → published_as doi:10.1093/nar/gkaa486`(bioRxiv 预印本正确解析到其 Nature 正式版)。
- **真实撤稿 + 端到端**:`forage fetch --query "retracted article cancer" --script crossref_verify.py --source pubmed` → ingest 8 条,`forage list` 全部标 `RETRACTED`,link 后 grade=`unsupported`。
- core 361(未改)+ acceptance 绿;`argument.rs` 空 diff;无新依赖(脚本仅用 Python 标准库 urllib)。

## 边界(诚实)

- `forage fetch` 以单一 `--source` 标签 ingest 整批;脚本面向预印本检索时用 `--source biorxiv`/`pubmed` 即可:撤稿压过一切、已发表经 `published_as` 解封、裸预印本按 source 封顶,均正确。
- 访问级别是启发式(preprint/有 license 视为 OA 全文);精确 OA 判定需 Unpaywall 等,留作后续。
- 网络在脚本里完成,受用户 egress 策略约束;AgentFlow 核心不发请求。
