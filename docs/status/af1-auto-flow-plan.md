# AF1 实现简报：无 --flow 时自动建 flow 跑完（修审计 🟠 A1 自治半途而废）

Status: Assigned to Codex
Owner: Claude(编排) · Codex(执行)
Spec source: 深度审计 2026-06-04 🟠 A1 —— 用户选定「(a) 自动建 flow 跑完，贴 §1.5 动态图默认自主」
Depends on: run_cycle apply 路径、graph_patch、approve_flow、auto_run_applied_step、L4、PV2 provenance —— 均在 main

## 背景（缺口）
`--apply --auto-run` 对无关联 flow 的新假设：循环正确起草步骤（gene=THRSP），但 apply 仅在 config.flow 为 Some 时执行（agent.rs:300），无 --flow 时 drafted_step 被静默丢弃、不 raise 决策、却报 outcome=advanced。违反 A1+A4。空 flow 被 approve_flow 拒绝（"flow must contain at least one step"）。

## 目标
config.apply 且有 drafted_step 但 config.flow 为 None 时，循环**自动创建一个含该步骤的 flow**（确定性 id、幂等），随后复用现有 provenance + auto-run + L4 全链，让自主链在无 --flow 时也跑完。人类决策仍在 L4 stance 解读处；trace checkpoint（循环已取）兜底可回退（A4）。

## 编排者裁决（约束）
1. **触发条件**：仅当 `config.apply && proposal.drafted_step.is_some() && config.flow.is_none()`。config.flow 为 Some 的现有路径**完全不变**（零回归）。config.apply 为 false 或无 drafted_step 时不触发。
2. **保留 A3 刹车**：自动建 flow 前仍走现有 `policy.assess(&ctx)` 闸门——若返回 kind（高代价/近预算等）则 raise 决策点而非自动建（与现有 graph_patch 路径同等对待）。只有无刹车才自动建+跑。
3. **确定性幂等 flow id**：`auto_<hypothesis_id>`。
   - 若该 flow 不存在 → 用 `FlowDraft{ id: auto_id, name, steps: [drafted_step] }` + `approve_flow` 创建（steps 非空满足校验）。step_id 为 drafted_step.id。
   - 若已存在（循环重跑）→ 走现有 `apply_branch_patch_for_proposal(auto_id, ...)` 把步骤 add 进去（复用现有 add_step patch）。
4. **抽公共 finalize helper**：把现有 graph_patch 分支里「emit_inferred_params_for_step + （config.auto_run 时）auto_run_applied_step + 收集 AppliedAction」这段下游编排抽成一个方法，**两条路径（graph_patch add_step / auto 建 flow）共用**，避免重复、保证 PV2 溯源与 L4 在两条路径都触发。
5. **可见（A4）**：自动建 flow 记进 cycle report——新增 `AppliedAction::FlowAutoCreated { flow_id }`（或等价），让 outcome 不再误导（有真实 apply+run 而非仅 lifecycle）。
6. **判决/PV1/PV2/L4 逻辑不变**：本步只补「无 flow→建 flow」入口 + 抽 helper；不改判决、校验、封顶、stance 逻辑。无新依赖/表；新增 AppliedAction 变体属附加（向后兼容序列化）。

## 交付物
- agent.rs：run_cycle apply 分支增「config.flow None 时自动建/复用 auto flow」；抽 finalize helper；新 AppliedAction::FlowAutoCreated。
- 测试：
  - 无 --flow + 自包含步骤（无 needs/inputs）→ 自动建 flow、apply、（auto_run 时）触发 auto-run/StanceAssessment；cycle 含 FlowAutoCreated。
  - config.flow Some 路径零回归（现有测试不改且通过）。
  - A3 刹车：near_budget 等触发时 raise 决策而非自动建。
  - 幂等：同假设重跑不重复建 flow（第二次走 add_step 或安全跳过）。

## 验收标准（Claude 逐条复核 + live 复跑）
- [ ] clippy -D warnings 干净；cargo test 全绿；acceptance 通过；fmt 通过。
- [ ] **live 复跑**：审计那条 THRSP 场景（无 --flow）现在自动建 flow → 跑出 TCGA 观测 → raise StanceAssessment（含 PV2 ⚠ 若 gene 是推断）；outcome 反映真实 run。
- [ ] 零回归：config.flow Some 路径不变；现有 core/cli 测试不改且通过。
- [ ] A3 刹车与幂等如期。
- [ ] 仅 agent.rs（必要时极小辅助）；无新依赖/表。

## 不在本里程碑
- 步骤有未满足 needs 时的多步 flow 脚手架（自包含步骤先行；有依赖的走现有 apply_failure 优雅记录）。
- A2 语义工具匹配、A3 flow list 命令（审计 🟡/🟢，独立后续）。

## 验收记录（Claude 独立复验 + live 实证 2026-06-04）
- ✅ clippy/test(core 256,+4)/acceptance/fmt 全绿；仅 agent.rs + CLI formatter；无新依赖/表。
- ✅ **A1 闭合（真 claude + 真 cBioPortal live）**：无 --flow 跑 THRSP 假设 → applied=[lifecycle_transition, flow_auto_created, step_run]、raised=[stance_assessment]、outcome advanced→**handed_off**；自动建 flow auto_<hyp> 有 completed run（真 TCGA: THRSP score -1.165）。
- ✅ **全链协作（live）**：AF1 自动建 flow 路径里 PV1 校验(gene=THRSP)、PV2 溯源(agent.params_inferred emit)+L4 digest ⚠ 告警、StanceAssessment 含真实发现，全部触发——finalize helper 在两条路径正确共用。
- ✅ 零回归：config.flow Some 路径不变；A3 刹车（near_budget→raise BudgetThreshold 不建 flow）+ 幂等（同假设重跑只建一次）有测试。
结论：合并就绪。深度审计 🟠 A1 闭合——自主链在无 --flow 的最常见路径也跑完，贴 §1.5 动态图默认自主，人类决策回到 L4 解读处，trace 兜底可回退。
