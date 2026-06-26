# 验证记录:文献状态自动核验(ingest 携带 retracted/published_as)

Date: 2026-06-26
Status: PASS — forage hits JSONL 现在可携带 `retracted` / `published_as`,所以外部核验脚本(查 PubMed/Crossref/Retraction-Watch)一填,撤稿/发表状态就**自动**流入观测并被诚实分级。核心保持离线;网络只在用户脚本里。

## 动机

#100/#102 给了撤稿与发表升级,但状态是**人工**标记(`forage observe --retracted/--published-as`)。要"自动核验",不能在核心里硬塞网络调用(会破坏 0-network 判决不变量 + 可测性)。AgentFlow 既有模式正合适:**外部脚本产出 hits JSONL → `forage ingest`/`forage fetch` 摄入**。而 #100 的 review 恰好标过一个 LOW:`parse_forage_hit` 忽略了 JSONL 里的 `retracted`。这一刀就把它补齐。

## 改动(均在 CLI 层,核心 forage.rs 不动)

- `ForageHit` + `parse_forage_hit` 读可选 `retracted`(bool,默认 false)与 `published_as`(string,缺省 None);新增 `json_bool_field` 解析器。`ingest_forage_hits` 把它们透传给 `record_forage_observation`(原来硬编码 `false, None`)。缺省字段保持原行为(向后兼容)。
- 例子脚本 `examples/forage/verify_status.py`:`forage fetch`/`--forage-script` 协议(`--query --max --out`)的核验模板,清楚标出"在此替换为真实 PubMed/Crossref/Retraction-Watch 查询",默认离线 stub 可确定性运行。

## 证据

- CLI 测试 `forage_ingest_carries_verified_retraction_and_publication_status`:一份带 `retracted:true` / `published_as` / 裸预印本三行的 JSONL → `forage ingest` → `forage list` 含 `"retracted":true` 与 `"published_as":...`;link 后三种 grade 分别为 `unsupported` / `literature_supported` / `hypothesis`。
- **Live**(端到端,经例子脚本):`forage fetch --query "SPP1 survival" --script examples/forage/verify_status.py --source biorxiv` → 摄入 3 条,`forage list` 显示 `RETRACTED`/`PUBLISHED` 标记 → link 后:
  - `PMID:RETRACTED → unsupported`
  - `doi:10.1101/published → literature_supported`(核验到已发表 → 解封)
  - `doi:10.1101/preprint → hypothesis`(裸预印本 → 封顶)
- `argument.rs` 空 diff;core 361 + cli(+1)+ 两个 acceptance 脚本(v1 + session)绿;clippy 干净;无新依赖。

## 边界(诚实)

- AgentFlow **不**自己发网络请求——核验在用户脚本里完成,受其 egress 策略约束;核心只摄入 + 诚实分级。
- 例子脚本默认是离线 stub;接真实 API 是用户/部署方的事(模板已标出位置)。
