# AS12 实现简报：证据引用可审计性（Robin 原则 A 收口，窄、通用、非破坏）

Status: Assigned to Codex（新分支 feat/citation-auditability，从 main 起，main 已含 AS7–AS11）
Owner: Claude(编排) · Codex(执行)
Spec source: 对照 Robin（Nature s41586-026-10652-y）"每条文献主张都带可核验引用" 的严谨性原则，编排者核查后定位的窄缺口
Depends on: AS1–AS11（已并入 main）

## 背景（已有基础 + 真缺口）

核查发现 AgentFlow 的"文献引用接地"链路**已基本建成**，不需重建：
- `examples/tools/pubmed_search.py`（example 工件）输出 `PMID:xxxx`；
- `forage.rs::link_forage_evidence` 已把 `source = PMID`、`grade = LiteratureSupported` 写入证据；
- `agent run` 默认 `auto_forage: true`；
- `report.rs` 的 research report 已渲染每条证据的 `source`（含 PMID）与 "Literature foraged" 段；
- `argument.rs::cross_modal_corroboration` 已做文献 vs 实证三角校验；
- LiteratureSupported 权重=1，self-deception gate 拦截"文献单独定论"。

**唯一真缺口（可审计性）**：
1. **决策点摘要不显引用**：`agent.rs::strong_verdict_digest`（1829）用 `evidence_ids(&preview.supporting)`（`agent.rs:2185`，只 join 内部事件 ID 如 `event_123`）。人在决策交接点看到的是不透明 ID，**看不到是哪篇文献（PMID/DOI）支撑了判决**。
2. **未引用的文献证据不被标记**：用户可 `evidence link --grade literature_supported`（`source` 为空或自由文本），没有任何信号提示"这条文献主张没有可核验 ID"。Robin 级严谨 = 文献主张必须可核验。

**核心原则（必须遵守）**：本里程碑只做**通用的引用识别 + 呈现 + 诚实标记**，**不**硬性拒绝、**不**改证据 schema/事件结构、**不**引入网络/LLM、**不**写入任何单次任务相关的具体文献/基因/PMID 常量（与既有"core 保持通用、不把单次任务代码当核心"原则一致）。沿用 AgentFlow 既有的 "⚠ 未验证" 诚实风格，对未引用文献证据加 "⚠ 未引用" 标记，而非删除或阻断。

## 编排者裁决（约束）

### 1. 通用引用识别(纯函数，确定性)

在 `crates/agentflow-core/src/argument.rs`（`EvidenceLink` 所在）新增一个**纯函数**，识别证据 `source` 中的可核验引用 token：

```rust
/// Recognize a verifiable citation token in an evidence source string.
/// Pure + deterministic: no LLM, no network. Recognizes PMID, DOI, and http(s) URLs.
pub fn recognized_citation(source: Option<&str>) -> Option<&str> {
    let s = source.map(str::trim).filter(|s| !s.is_empty())?;
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("pmid:") || lower.starts_with("doi:")
        || lower.starts_with("http://") || lower.starts_with("https://") {
        Some(s)
    } else {
        None
    }
}
```
（实现可微调，但语义固定：识别 `PMID:`/`DOI:`/`http(s)://` 前缀，大小写不敏感；其余视为无可核验引用。不得引入正则之外的新依赖；用标准库字符串判断即可，无需 regex crate。）

**不改 `argument.rs` 的判决逻辑/`render_verdict`/权重/gate**——只新增这个纯函数。验证 `argument.rs` 仍 0 处 LLM/网络调用。

### 2. 决策摘要显引用(agent.rs)

在 `crates/agentflow-core/src/agent.rs` 新增 `evidence_citations(evidence: &[EvidenceLink]) -> String`（与 `evidence_ids` 并列，2185 附近）。逐条渲染：
- 有可核验引用 → 该引用（如 `PMID:12345`）；
- `grade == LiteratureSupported` 且无可核验引用 → `⚠未引用`；
- 其余（observed/inferred 等计算所得，无引用）→ 退回其 `link.id`（保持可追溯）。
- 空集 → `none`（与 `evidence_ids` 一致）。

把 `strong_verdict_digest`（1829）中**支持/反证两处**从 `evidence_ids(...)` 改为 `evidence_citations(...)`，使决策点人类看到引用而非裸 ID。其余用到 `evidence_ids` 的地方（如 `cycle_completed_payload_json` 的内部 payload）**保持不变**（那是机器可追溯字段，不是人类审计文本）。

### 3. 报告标记未引用文献(report.rs)

`crates/agentflow-core/src/report.rs::write_research_evidence_group`（632）每条证据当前渲染 `- [{grade}] {note} - source: {research_evidence_source(link)}`。增强：当 `link.grade == LiteratureSupported` 且 `recognized_citation(link.source.as_deref())` 为 `None` 时，行尾追加 ` ⚠未引用`。其余不变。`research_evidence_source` 保持原样（仍显示 source 或 observation_id 或 "-"）。

### 4. 不变量与约束

- 不改证据/事件 payload 结构、不改 `EvidenceGrade`/`Stance`/`DecisionKind`、不改 allowlist、不改 AS7–AS11 逻辑。
- 不引入硬性校验/拒绝（非破坏：`source=None` 的既有 LiteratureSupported 证据仍可链接，只是渲染 `⚠未引用`）。
- 不引入新 Rust 依赖（标准库字符串判断，不用 regex）。
- core 仍 0 LLM/网络依赖；`argument.rs` 仍 0 处 LLM/网络。
- **不写入任何具体 PMID/基因/疾病常量**到核心代码。

## 测试

- `argument.rs`：`recognized_citation` 纯函数单测——`PMID:123`/`doi:10.x`/`https://...` 识别；自由文本 `"trust me"`/空/`None` → `None`；大小写不敏感。
- `agent.rs`：`evidence_citations` 单测——混合证据集（带 PMID 的文献、无引用的 LiteratureSupported、observed 计算证据、空集），断言渲染分别为引用 / `⚠未引用` / `link.id` / `none`；并断言 `strong_verdict_digest` 输出包含 `PMID:` 与/或 `⚠未引用`。
- `report.rs`：research report 单测——LiteratureSupported 且无引用的证据行含 `⚠未引用`；带 `PMID:` 的不含该标记。
- 既有断言精确字符串的 digest/report 测试若因新增标记/改 ID→引用而失败，按新语义更新（不得为了过测试而削弱语义）。
- core 测试数预期 +3~5。

## 验收标准（Claude 复核）

- [ ] fmt / clippy / core / cli / `scripts/acceptance-v1.sh` 全绿；`argument.rs` 仍 0 处 LLM/网络。
- [ ] 单测证明 `recognized_citation` 通用识别正确。
- [ ] 决策点 `strong_verdict_digest` 显示文献引用(PMID/DOI/URL)，未引用文献显示 `⚠未引用`。
- [ ] research report 对未引用 LiteratureSupported 证据加 `⚠未引用` 标记。
- [ ] 无新依赖；无具体单次任务常量进入核心；core 测试数不减少。

## 不在本里程碑

- 不做硬性引用校验/拒绝(保持非破坏；如需"必须带引用才能影响判决"是更大的策略改动，另议)。
- 不做原则 B(report 的 Methods & Code 复现段)/原则 C(机制子假设 spawn)/原则 D 的候选排序——另立路线图。
- 不改 `examples/tools/pubmed_search.py`(它是 example 工件，被引擎通用消费，不动)。
