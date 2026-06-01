# T2 实现简报：主循环接工具选择（提议模式）

Status: Implemented + verified (2026-06-01)
Date: 2026-06-01
Owner(orchestrator): Claude · Executor: Codex
Spec source: T1 后续连线（让分支提议变成带具体 step 的可落地提议）
Depends on: H7a（agent.rs）、T1（tool_select.rs）、H2（branch::ProposedStep）—— 均已合并 main

## 验收记录（Claude 独立复验 2026-06-01）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **167**（基线 165，+2）/ cli 43 / schemas 3 全绿。
- ✅ Cargo 零变更；改动**严格局限** `agent.rs`（290/7）+ `agent_ops_commands.rs` 的 `agent run`（39/3）；tool_select.rs / branch.rs / hypothesis.rs / argument.rs **零改动**。
- ✅ 仍提议模式：`agent.rs` 无 `apply_graph_patch` / `transition_hypothesis` / `propose_branch_patch`。
- ✅ 新增 `EnrichedProposal`，`CycleReport.branch_proposals` 改为 `Vec<EnrichedProposal>`；Deepen/Spawn 提议自动 `match_tools` + `draft_step_for`。
- ✅ **端到端冒烟**：注册 marker 工具 + 导入 TSV 工件 + 弱证据假设 → `agent run` 提议 `deepen ... matched tool: marker/marker_survival_scan (medium)`，`--json` 的 EnrichedProposal 含 matched_tool/fit/drafted_step。

结论：合并就绪。循环现产出「带具体工具+step」的真提议；H7b-2 自主 apply 的「有补丁可咬」前置彻底就位（开闸仍待显式授权）。

## 目标

让 `run_cycle` 为每个 Deepen/Spawn 分支提议**自动匹配工具并草拟具体 `ProposedStep`**，使提议从抽象变为可落地。**仍是提议模式**：不调用 `propose_branch_patch`（它需 flow 上下文）、不 apply、不改生命周期。改动**局限在 `agent.rs`**（循环自身结构）+ `agent run` CLI 输出增强。

## 硬约束

1. 改动局限 `agent.rs`（循环结构/逻辑/to_json/测试）+ `agent_ops_commands.rs` 的 `agent run` 输出（additive）。**不改** tool_select.rs / branch.rs / 其它已验收模块逻辑（只调用其公开 API）。
2. **仍提议模式**：禁止 `apply_graph_patch` / `transition_hypothesis` / `propose_branch_patch`。本步不引入任何写图/写生命周期。
3. 不新增依赖；不新增表/event_type（`agent.cycle_completed` 沿用，payload 可 additive）。
4. 质量门全绿：`clippy -D warnings` + `cargo test`。基线 core 165 / cli 43 / schemas 3。
5. 关键词分词为确定性启发式（见下），保证测试可复现。

## 交付物（`crates/agentflow-core/src/agent.rs`）

新增：
```rust
pub struct EnrichedProposal {
    pub decision: BranchDecision,
    pub matched_tool: Option<String>,            // top 候选 tool_ref；无候选则 None
    pub matched_fit: Option<String>,             // Fit::as_str
    pub match_reason: Option<String>,            // 来自 ToolCandidate.reason
    pub drafted_step: Option<crate::branch::ProposedStep>,
}
// EnrichedProposal::to_json
```

修改 `CycleReport`：
```rust
pub branch_proposals: Vec<EnrichedProposal>,  // 原为 Vec<BranchDecision>
```
（同步更新 `CycleReport::to_json` 与 agent.rs 内的相关测试断言。）

`run_cycle` 选分支阶段（Deepen/Spawn 处）改为：
1. 构造 `CapabilityQuery`：
   - `keywords`：对 `decision.candidate.statement` 分词——小写、按非字母数字切分、保留长度 ≥4 的 token、去重、最多取前 8 个（确定性）。
   - `available_input_types`：`list_artifacts()` 的 `artifact_type` 去重。
   - `desired_output_type`：`None`（无法从假设可靠推断）。
2. `match_tools(&query)`；取 top 候选（若候选列表非空）。
3. 若有 top 候选：`draft_step_for(top.tool_ref, &available)`（`available` = `list_artifacts()` 的 `(artifact_type, id)` 列表）→ 填 `EnrichedProposal` 的 matched_tool/fit/reason/drafted_step。
4. 无候选：`EnrichedProposal` 的工具/步骤字段为 `None`（提议仍保留，只是没匹配到工具）。
5. push `EnrichedProposal`（替代原来 push `BranchDecision`）。

> 全程不 apply、不 propose_branch_patch、不改生命周期。`propose_branch_patch(flow, decision, step)` 仍是用户/后续显式动作。

### `agent run` CLI（`agent_ops_commands.rs`，additive）

人类输出在每个 branch proposal 行后追加：matched tool + fit（无则显示 "no tool match"）。`--json` 输出 `EnrichedProposal`（含 drafted_step）。不改其它命令。

## 验收标准（Claude 审核逐条核对）

- [ ] `clippy -D warnings` 无警告；`cargo test` 全绿，core/cli 较基线净增。
- [ ] 无新依赖/表/event_type；grep 确认 agent.rs 仍无 `apply_graph_patch`/`transition_hypothesis`/`propose_branch_patch`。
- [ ] 只改 agent.rs + agent_ops_commands.rs（`agent run`）；tool_select.rs/branch.rs/其它模块逻辑未改。
- [ ] 行为测试：①注册一个工具 + 导入匹配类型工件 + 造一个会产生 Deepen/Spawn 的假设 → 该提议的 `matched_tool` 非空且 `drafted_step` 合法；②无匹配工具时 `matched_tool=None` 但提议仍在；③关键词分词确定性（给定 statement → 固定 query）。
- [ ] `agent run` 既有行为（provisional/strong/abandon 路径）回归不变；现有 agent.rs 测试相应更新但语义不变。

## 不在本里程碑（明确排除）

调 `propose_branch_patch` 落补丁、auto-apply（→ H7b-2，需显式授权）、flow 上下文绑定、output 类型从假设推断、关键词的语义/同义扩展。
