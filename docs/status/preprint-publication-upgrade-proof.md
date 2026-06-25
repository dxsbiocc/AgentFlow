# 验证记录:证据广度 — 预印本→正式发表升级(解封 preprint cap)

Date: 2026-06-25
Status: PASS — foraged 预印本可标记"已正式发表",发表后**解除 #99 的 Hypothesis 封顶**,按访问级别正常分级(全文 → `LiteratureSupported`)。与撤稿(#100)对称,补全文献来源的可信度生命周期。

## 动机

#99 把预印本全文封顶到 `Hypothesis`(未经同行评议)。但预印本会**正式发表**——一旦经同行评议,就不该再被封顶。这是撤稿(降级)的镜像:发表是**升级**。补上后,同一来源的可信度生命周期完整:**预印本(`Hypothesis`)→ 发表(`LiteratureSupported`)→ 撤稿(`Unsupported`)**。

## 改动

- `ForageObservation` + payload 加 `published_as: Option<String>`(发表版本的 id,如 `PMID:…`);两处都 `#[serde(default, skip_serializing_if = "Option::is_none")]` → 旧事件解析为 `None`,且未发表时不上 JSON 线(向后兼容 + 不污染既有 `to_json` 断言)。
- `record_forage_observation` 增 `published_as: Option<&str>` 参数。
- `grade_for_forage_observation` 优先级:**撤稿 → `Unsupported`(压过一切)→ 已发表 → 按 access 分级(解封)→ 否则预印本封顶**。
- CLI:`forage observe --published-as <id>`;`forage show` 显示 `Published as:`;`forage list` 标 `PUBLISHED`(与 `RETRACTED` 对称)。auto-forage ingest 默认 `None`。

诚实联动:撤稿压过发表——"已发表但被撤稿"仍 `Unsupported`。

## 证据

- 单测 `published_preprint_is_no_longer_capped`:bioRxiv 全文 `Hypothesis` → 加 `published_as` 后 `LiteratureSupported` → 再加 `retracted` 后 `Unsupported`(撤稿压过发表)。
- CLI 测试 `forage_observe_published_preprint_lifts_cap`:`--published-as` → `forage link` 得 `literature_supported`,`forage list` 显示 `PUBLISHED`。
- **Live**:同一 bioRxiv 全文三态 —— 预印本 `hypothesis` / 已发表 `literature_supported` / 发表后撤稿 `unsupported`。
- `argument.rs` 空 diff;core **361**(+1)+ cli(+1)+ acceptance 绿;clippy 干净;无新依赖。

## 边界(诚实)

- `published_as` 是**人工/agent 标记**(`--published-as`),未自动核验该预印本是否真已发表——后续可接 DOI/PubMed 核验。
- 事件溯源下,"升级"通过重新 observe(带 `--published-as`)记录;未做独立的 `forage supersede` 链接旧预印本观测。
