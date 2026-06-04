# LD1 实现简报：跨模态印证可见（修审计 🟡 文献/数据孤岛，B 选其一：先做可见）

Status: Assigned to Codex
Owner: Claude(编排) · Codex(执行)
Spec source: 深度测试审计 🟡「文献/数据孤岛」—— 用户选定「跨模态印证可见（先做）」
Depends on: F3（report research 统一研究报告）、H6/R1（forage 文献证据 LiteratureSupported）、G1/S2（数据观测证据 Observed/Inferred）、argument（证据账本）—— 均已合并 main

## 背景与诊断
判决引擎已汇流两类证据（文献 LiteratureSupported=1 加分但不能独立 affirm；数据 Observed/Inferred 才能 affirm）。孤岛**不在汇总**，在于**两类证据是否印证/冲突没有被显式呈现**——读报告的人看不出「文献说支持、但数据反驳」这种关键科研信号。本里程碑把它**可见、可审计**，但**不自动驱动动作**（最低风险、最诚实 A4）。

## 模态判定（可靠信号，纯由 grade）
- **文献证据**：`grade == LiteratureSupported`（仅 forage 的 grade_from_access 产生）。
- **数据/经验证据**：`grade ∈ {Observed, Inferred}` 且 `observation_id.is_some()`（仅工具观测产生；PV2 封顶后仍在此集）。
- 其余（Hypothesis/Unsupported）：非任一模态，不计入跨模态判定。

## 目标
给每个假设计算并呈现「文献证据 ↔ 数据证据」的跨模态印证状态，渲染进 `report research`。**纯读侧派生**：不改判决逻辑、不加事件/表/依赖。

## 编排者裁决（约束）
1. **纯函数分类器**（放 argument.rs 或 report.rs，确定性、无副作用）：
   `cross_modal_corroboration(evidence: &[EvidenceLink]) -> CrossModalAssessment`
   - 按上面模态判定 + stance，算出：lit_support/lit_contra/emp_support/emp_contra 是否存在。
   - 状态（CrossModalStatus）：
     - `Conflicting`：(文献支持 & 数据反驳) 或 (文献反驳 & 数据支持)——**优先级最高，任一跨 stance 不一致即冲突**。
     - `Corroborated`：两模态在同一 stance 都有证据（都支持 或 都反驳），且无冲突。
     - `EmpiricalOnly`：只有数据证据。
     - `LiteratureOnly`：只有文献证据。
     - `None`：两类都无。
   - 返回结构含 status + 简短计数明细供渲染。
2. **渲染进 report research**：每假设段加一行「跨模态印证：<人类可读>」，如：
   - 冲突 → `跨模态印证：⚠ 冲突（文献支持 / 数据反驳）`
   - 印证 → `跨模态印证：印证（文献 + 数据一致支持）`
   - 仅数据 / 仅文献 / 无 → 对应文案。
3. **判决/证据逻辑零改动**：本步只读 evidence 派生展示，不影响任何 verdict 结果（render_verdict、score、cap 全不动）。
4. **零回归**：除 report research 新增该行外，无其它输出变化；现有测试中断言 report research 文本的相应更新（新增行属预期），其它测试不改且通过。无新依赖/事件/表。verdict --json 等其它输出不变（本步不动它们）。

## 交付物
- argument.rs（或 report.rs）：`CrossModalStatus` 枚举 + `CrossModalAssessment` + `cross_modal_corroboration` 纯函数。
- report.rs：report research 每假设渲染跨模态行。
- 测试：分类器五状态各一例（尤其冲突：lit supports + data contradicts）；report research 含该行；无证据/单模态文案；现有非 report-research 测试不改且通过。

## 验收标准（Claude 逐条复核）
- [ ] clippy -D warnings 干净；cargo test 全绿；acceptance 通过；fmt 通过。
- [ ] 分类器五状态正确（单元测试）；冲突优先级正确。
- [ ] report research 渲染跨模态行（行为测试：构造 文献支持+数据反驳 → 报告显示「冲突」）。
- [ ] **零回归**：verdict 结果不变（判决逻辑未动）；除 report research 新增行外无其它输出变化；无新依赖/事件/表。
- [ ] 仅 argument.rs/report.rs（必要时极小辅助）；core 其它模块未动逻辑。

## 不在本里程碑
- 文献→数据 / 双向自动播种（更强形态，后续若需再做）。
- verdict --json 增跨模态字段（如需机器可读再单独加）。
- 跨模态印证影响判决分数（保持判决不被展示层污染）。

## 验收记录（Claude 独立复验 2026-06-04）
- ✅ clippy -D warnings 干净；cargo test core 252(+7)/cli 57+2/schemas 3 全绿；acceptance 通过；fmt 通过。
- ✅ **判决不变量守住**：render_verdict/score_for/capped_evidence_grade/has_observed 零 diff——跨模态印证是纯读侧派生，不污染判决。
- ✅ 分类器实现即规格：模态纯由 grade（文献=LiteratureSupported；实证=Observed/Inferred 且 observation_id.is_some()），冲突优先级最高。
- ✅ 冲突路径由集成测试 generate_research_report_markdown_renders_cross_modal_conflict 证明（走 report research 真实实现，断言「⚠ 冲突（文献支持 / 数据反驳）」）；五状态 + 单模态/无证据文案均有测试。
- ✅ 设计复核：实证模态要求 observation_id 背书是有意为之（无观测背书的手填 observed 不算实证印证，契合反自欺），非缺陷。
- ✅ 仅 argument.rs/report.rs；无新依赖/事件/表/schema。
结论：合并就绪。审计 🟡 文献/数据孤岛「跨模态印证可见」面闭合——report research 显式呈现文献证据与数据证据的 印证/冲突/单模态，重点凸显冲突，判决逻辑不受影响。更强形态（文献→数据自动播种）留作可选后续。
