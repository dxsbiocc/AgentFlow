# 简报：自治 needs-edge 接线(P2,确定性 provenance 推断)

Status: Assigned to Codex（worktree /tmp/af-p22，branch feat/p22-needs-wiring，从 main 起）
RFC: docs/design/agent-scheduling-design.md §4.2(自治依赖接线)。让 agent 自己长出可调度的多步图,而非只跑人写的 flow。

## 现状 / 缺口

- 应用一个 proposed step 时,`flow_step_draft_from_proposed`(agent.rs:2064)直接 `needs: step.needs.clone()` —— 而 enrich 路径产出的 ProposedStep.needs 通常为空。
- 结果(README "not wired"):agent 加入的 step 即使消费了上游 step 的输出,也**不接 needs 边** → 多步图无法被 agent 自动正确编排(调度器 P2.1a 只能看到人写的边)。

## 目标(确定性,0-LLM)

应用 step 时,从**输入的来源(provenance)**确定性推断 needs 边:计算工件带 `source_step_id`(`ArtifactSummary.source_step_id`),若某输入来自本 flow 内某个 producer step,则消费 step `needs` 那个 producer。无 LLM、无网络——纯查既有 provenance。

## 实现要求

1. 新增确定性函数(core,agent.rs 或合适处),签名约:
   `fn infer_step_needs(&self, flow_id, inputs: &[(String,String)], existing_needs: &[String]) -> Result<Vec<String>, StorageError>`
   规则,对每个 `(port, value)` 输入:
   - 若 value 形如 `<producer_step>.<output>`(组合引用语法,见 runtime resolve_step_output)→ producer_step 是一条 needs(显式组合,规范成 local_id)。
   - 否则若 value 是 artifact id → 查该 artifact 的 `source_step_id`;若 `Some` 且该 step 属于 `flow_id` → 加入其 local_id。
   - 合并 existing_needs,**去重**;**排除自指**(step 不能 need 自己);保持确定性顺序(如按 local_id 排序或按输入顺序稳定)。
2. 接线:`flow_step_draft_from_proposed` / `apply_branch_patch_for_proposal` 路径,在生成 FlowStepDraft 前用 `infer_step_needs` 算出 needs(并入 step.needs)。auto-flow 首 step(新建 flow、无上游)推断结果自然为空,行为不变。
3. **只加真实 provenance 边,绝不臆造**:仅当 artifact 确有 source_step_id 且 producer 在本 flow 内才加边。

## 不变量(硬约束)

- `git diff crates/agentflow-core/src/argument.rs` 为空;推断 0-LLM/0-网络(只读 artifact registry)。
- 向后兼容:显式 needs 保留;推断只**增加**有 provenance 依据的边;不产生环(provenance 指向先前已存在的 producer,天然上游;加自指/不存在 step 的守卫)。
- 单链/首-step/无 provenance 输入 → 推断为空 → 现有行为不变;现有测试断言不改即过。
- 仅改 `crates/agentflow-core`(agent.rs + 必要的 artifact 查询);不碰 argument.rs、不碰 CLI schema。无新依赖。

## 测试(离线,合成)

- 单测 `infer_step_needs`:
  - 输入用 `<producer>.<out>` 语法 → needs 含 producer。
  - 输入是 computed artifact(source_step_id 指向本 flow 内 step)→ needs 含该 step。
  - 输入是 imported artifact(source_step_id=None)→ 不加边。
  - 显式 needs + 推断去重;排除自指。
- 集成(端到端,验证"agent 接线后能正确调度"):构造一个场景——step A 产出 computed artifact,step B 的输入引用该工件,经 apply 路径加入 B → 断言 B 的 needs 含 A(边被推断),且 run_flow 按 A→B 顺序执行成功(与调度器 P2.1a 协同)。可仿 success_path_affirm.rs / 现有 graph_patch 测试风格,用合成本地工具。
- 跑:`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test -p agentflow-core`、`bash scripts/acceptance-v1.sh`。**不要** `cargo test --workspace`。

不要 commit。报告:infer_step_needs 位置与规则、接线点、确认只加 provenance 边/不臆造/不成环、argument.rs 未动、现有测试未改即过、acceptance 绿。
