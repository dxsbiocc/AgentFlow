# F3 实现简报：统一研究报告（report research）

Status: Implemented + verified (2026-06-02)
Date: 2026-06-02
Owner(orchestrator): Claude · Executor: Codex
Spec source: 待完善清单 #3（高优先）—— 把控制层产出渲染成「结论+证据+不确定性」报告
Depends on: H1/H5（hypothesis/verdict/evidence）、H3（decision）、H6（forage）—— 均已合并 main

## 验收记录（Claude 独立复验 2026-06-02）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **180**（基线 176，+4）/ cli **49**（+1）/ schemas 3 全绿。
- ✅ additive：`report.rs` 新增 `generate_research_report_markdown`（`generate_report_markdown` 未改，唯一删除为 `use` 导入行重排）；CLI `report research` 分发，`report <flow-id>` 回归通过；`VerdictSummary` 加可选 `frontier`（纯加）。无新依赖/表/event_type。
- ✅ **§15 合规**：证据按 stance 分组并显式标 grade（`[observed]`/`[hypothesis]` 等）；provisional→needs stronger evidence、fundamental→undecidable+frontier、无 verdict→not yet evaluated。
- ✅ **端到端冒烟**：`report research` 渲染假设/分级证据/待决策/文献/笔记；**诚实性**——强判决未持久化时报告显示「(no verdict) / not yet evaluated」并把「预览 affirmed 需人类 gate」列入 Open Decisions，不把弱/未定论呈现为已确立结论。

结论：合并就绪。「输入课题 → 检索 → 假设/证据/判决 → 自治推进 → 带证据链的结论报告」端到端闭环齐备。

## 目标

新增项目范围的 **研究报告** `report research`：把假设、三态判决、**分级证据链**、待决策点、已拉文献、研究笔记渲染成一份 Markdown「研究结论 + 证据 + 不确定性」报告。让自治产出从散落各命令收敛为一份可读结论。现有 `report <flow-id>` 因果报告**不动**。

## 编排者裁决（约束）

1. **additive**：core 在 `report.rs` 新增 `generate_research_report_markdown(&self) -> Result<String, StorageError>`（不改现有 `generate_report_markdown`）；CLI 在 `report` 下新增 `research` 子命令（`report <flow-id>` 行为不变）。
2. 只读投影，复用现有 API：`list_hypotheses` / `evidence_for` / `latest_verdict_for` / `list_decision_points`（取 pending）/ `list_forage_observations` / `list_research_notes`。不新增写操作、event_type、表、依赖。
3. **§15 合规**：报告必须**区分证据 grade**（observed / inferred / literature_supported / hypothesis / unsupported），并对 provisional/inconclusive 明确标注「不确定/未决」，不得把弱证据呈现为已确立结论。
4. 风格对齐现有 CLI / report.rs（Markdown 生成、`#[cfg(test)] mod tests`）。

## 交付物

### core：`report.rs::generate_research_report_markdown`

项目范围 Markdown，建议结构：
```markdown
# AgentFlow Research Report
Generated: <unix ts>

## Hypotheses (N)
### <statement>
- id: <hyp_id>
- lifecycle: <proposed|under_test|...>
- verdict: <affirmed|refuted|inconclusive(provisional|fundamental)|(no verdict)>  (confidence: <low|medium|high>)
- supporting evidence (N):
  - [<grade>] <note> — source: <source|observation_id|->
- contradicting evidence (N): ...
- context/neutral evidence (N): ...
- uncertainty: <若 provisional → "evidence below decision margin / needs stronger evidence"；
                若 fundamental → "currently undecidable: <frontier>"；若 no verdict → "not yet evaluated">

## Open Decisions (pending) (N)
- [<kind>] <digest>  → options: <labels>

## Literature foraged (N)
- [<access_status>] <title> (<external_id>)

## Research notes (N)
- <question> → <finding> [<confidence>]
```
- 各 section 为空时输出友好占位（如 "No hypotheses recorded"）。
- 证据按 stance 分组（supporting / contradicting / neutral），每条显式标 grade。

### CLI：`report research`
- `report research [--path <p>]` → `generate_research_report_markdown()`（Markdown 到 stdout）。
- `report_command` 分发：首参 == `"research"` → 研究报告；否则按现状视为 `<flow-id>` 走 `generate_report_markdown`（**现有行为与测试不变**）。
- usage 追加 `report research` 行。

## 验收标准（Claude 审核逐条核对）

- [ ] `clippy -D warnings` 无警告；`cargo test` 净增，全绿；现有 report flow 测试与全部既有测试未改且通过。
- [ ] 无新依赖/表/event_type；`generate_report_markdown` 未改。
- [ ] 报告测试：①空项目 → 各 section 友好占位；②一个带证据+verdict 的假设 → 渲染 lifecycle/verdict/confidence/分组分级证据/uncertainty；③一个 pending 决策点 → 出现在 Open Decisions；④forage 观察与 research note 各渲染一条。
- [ ] §15：报告区分 grade（测试断言出现 `[observed]`/`[hypothesis]` 等标记）；provisional/fundamental 的 uncertainty 文案正确。
- [ ] CLI：`report research` 正确分发；`report <flow-id>` 仍走原路径（回归测试）。

## 不在本里程碑（明确排除）

JSON/HTML 导出、持久化报告工件、报告内的引用编号/参考文献区、跨项目聚合、把研究报告并入 flow 因果报告。
