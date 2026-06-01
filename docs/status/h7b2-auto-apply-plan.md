# H7b-2 实现简报：自主 apply 开闸（opt-in，默认关）

Status: Implemented + verified (2026-06-01)
Date: 2026-06-01
Owner(orchestrator): Claude · Executor: Codex
Spec source: [`agentflow-agent-control-layer-design.md`](../agentflow-agent-control-layer-design.md) §9-H7（H7b 的自治开闸半部）
Depends on: H7a/T1/T2（循环+工具选择）、H3（刹车）、H4/H7b-1（真回退）、H5（防自欺）—— 均已合并 main

## 验收记录（Claude 独立复验 2026-06-01）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **172**（基线 167，+5）/ cli **44**（基线 43，+1）/ schemas 3 全绿。
- ✅ 改动局限 `agent.rs`（486/12）+ `agent run` CLI + usage；无新依赖/表/event_type。
- ✅ **逐行核验**：4 处 apply 写操作（`transition_hypothesis` 234、`propose_branch_patch`/`approve_graph_patch`/`apply_graph_patch` 315-317）**全部在 `if config.apply` 分支内**，且各经 `DefaultPolicy::assess` 放行后才执行。
- ✅ **强判决（Affirmed/Refuted/Fundamental）与 Abandon**：`--apply` 下仍一律 `raise_decision_point`，无 apply 路径。
- ✅ `--max-apply` 经 `near_budget` 触发 brake 转 raise。
- ✅ **端到端资本冒烟**：①默认（无 --apply）status 仍 proposed（零行为变化）；②`--apply` 下 Provisional+Proposed 自动 →UnderTest；③`trace revert` 到 pre-apply checkpoint 回滚该 transition（status 退回 proposed）；④强证据 + `--apply` 仍 handed_off 且 `verdict show`=none（**绝不自动 affirm**）。

结论：合并就绪。自主 apply 以 **opt-in 默认关** 落地，A1/A2/A3/A4 + 防自欺在 `--apply` 下同时成立。开启自治 = 用户显式传 `--apply`。

## 目标

为控制循环加**默认关闭的自主 apply 能力**：`agent run --apply`。不传 `--apply` 时**行为与现在逐字节相同**（提议模式）。传 `--apply` 时，循环在**刹车放行**的安全信封内自主落地动作，并受**上限 + checkpoint 可回退 + 防自欺**三重约束。

## 不可违反的安全契约（违反即返工）

1. **默认关 = 零行为变化**：不传 `--apply`，所有路径与现状完全一致；现有 167 core 测试**必须不改且通过**（首要回归）。
2. **强判决永不自动**：Affirmed / Refuted / Inconclusive(Fundamental) 仍一律 `raise_decision_point` 交接人类（防自欺，A2）。`--apply` 下也不例外。
3. **Abandon 永不自动**：放弃分支仍 raise 交接，绝不自动 `transition` 到 Contradicted/Superseded。
4. **刹车门控**：每个候选 apply 动作先经 `DefaultPolicy::assess(StepContext)`；返回 `Some(kind)` → 改为 raise 交接；`None` → 才允许 apply。
5. **可回退**：循环开始即 `create_checkpoint`（已有），所有自动 apply 必须在该 checkpoint 之后，确保 `trace revert` 可整体回滚。
6. **上限**：`--max-apply <n>`（默认 5）封顶单轮自动 apply 次数，防失控。

## 交付物（`agent.rs` + `agent run` CLI）

### `agent run` 新增 flag
- `--apply`（bool，默认 false）：开启自主 apply。
- `--flow <flow-id>`（可选）：提供后才允许图补丁 apply（无 flow 则只做 flow-independent 的生命周期推进）。
- `--max-apply <n>`（默认 5）。

### `run_cycle` 增加 apply 路径（仅当 apply 开启时生效）

签名可改为内部带 `ApplyConfig { apply: bool, flow: Option<String>, max_apply: u32 }`；默认 `apply:false` 即现行为。新增 apply 动作：

1. **生命周期推进（flow-independent）**：对刚落库 Provisional 判决、且当前状态为 `Proposed` 的假设：
   - `StepContext { cost: Cheap, reversible: true, equivalent_branches: false, conflicts_user_premise: false, mutates_goal: false, near_budget: <已达 max_apply> }`
   - `assess` 返回 `None` → `transition_hypothesis(id, UnderTest, 保持原 confidence)`；否则 raise。
2. **图补丁 apply（需 `--flow`）**：对每个带 `drafted_step` 的 Deepen/Spawn `EnrichedProposal`：
   - `StepContext { cost: Moderate, reversible: true, equivalent_branches: <该假设有多个 high/medium fit 候选>, conflicts_user_premise: false, mutates_goal: false, near_budget: <已达 max_apply> }`
   - `assess` 返回 `None` → `propose_branch_patch(flow, decision, drafted_step)` → `approve_graph_patch(id)` → `apply_graph_patch(id)`；返回 `Some` → raise 交接。
   - 无 `--flow` → 不做图补丁 apply（仍记为提议，且不 raise）。
3. 每次成功 apply 计数；达到 `max_apply` 后，后续候选一律走 raise（通过 `near_budget=true` 自然触发）。
4. **强判决 / Abandon 分支**：维持现有 raise 逻辑，`--apply` 不改变它们。

### CycleReport 增加
```rust
pub applied: Vec<AppliedAction>,   // 记录本轮自动落地的动作
pub enum AppliedAction {
    LifecycleTransition { hypothesis_id: String, to: String },
    GraphPatchApplied { flow_id: String, patch_id: String, step_id: String },
}
// AppliedAction::to_json；CycleReport::to_json 增加 applied 字段（additive）
```
`agent run` 人类输出增加「Applied」段；`--json` 含 `applied`。

## 验收标准（Claude 审核逐条核对）

- [ ] **回归**：不传 `--apply` 时行为零变化，现有 167 core 测试未改且通过（首要）。
- [ ] `clippy -D warnings` 无警告；`cargo test` 全绿，净增测试。
- [ ] 无新依赖/表/event_type。
- [ ] 行为测试：①`--apply` 下 Provisional+Proposed 假设被自动 `transition` 到 UnderTest；②`--apply --flow` 下带 step 的 Deepen 提议被 propose→approve→apply（flow 中出现新步骤）；③强判决/Abandon 在 `--apply` 下**仍 raise、未自动落地**；④`--max-apply 1` 时第二个候选被 raise（near_budget）；⑤apply 后 `trace revert` 到循环起始 checkpoint 能回滚自动落地的状态。
- [ ] grep 确认：apply 相关写操作（transition/propose/approve/apply）**只在 apply 开启分支内**被调用；默认分支无这些调用。

## 不在本里程碑（明确排除）

自动设定 step 依赖边（needs 仍为空 = 新增独立步骤）、多目标/多 flow 编排、自动 forage 拉证据、把 `--apply` 设为默认（永远需用户显式传）。
