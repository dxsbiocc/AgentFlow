# H7a 实现简报：控制主循环（提议模式）

Status: Implemented + verified (2026-06-01)
Date: 2026-06-01
Owner(orchestrator): Claude · Executor: Codex
Spec source: [`agentflow-agent-control-layer-design.md`](../agentflow-agent-control-layer-design.md) §8 / §9-H7
Depends on: H1–H6 + C1/C2（全部已验收）

## 验收记录（Claude 独立复验 2026-06-01）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **155**（基线 151，+4）/ cli **35**（基线 34，+1）/ schemas 3 全绿。
- ✅ 新建 `agent.rs`（flat）；`agent` 模块无 `apply_graph_patch` / `transition_hypothesis`；新 event_type 仅 `agent.cycle_completed`；无新表/依赖。
- ✅ **H1–H6 已验收模块零改动**（`git diff --numstat` 全空，循环完全自包含）；改动仅 `agent.rs` + core/cli lib.rs 注册 + `agent_ops_commands.rs` additive `agent run`。
- ✅ `run_cycle` 四步逻辑：Provisional→`render_verdict(None)` 落库；强判决→预览不落库、`raise_decision_point(DeepenOrStop)`；Abandon→`raise_decision_point(GoalMutation)`；结局 HandedOff/Advanced/Idle。
- ✅ **端到端冒烟**：弱证据→`advanced`；强 observed 证据→预览 Affirmed 但**不落库**、raise 决策点（digest 含证据凭证 + 需人类 gate）、`handed_off`；`verdict show` 确认未自动 affirm。完整体现 A1 自主推进 + A2/A3 撞强判决即交接 + 防自欺 + 提议零不可逆。

结论：合并就绪。自动 apply 开闸 + 回退区间接入域投影 = H7b（需用户显式授权开闸）。

## 目标

把四引擎编排成一个**提议模式**的控制主循环 `agent::run_cycle`：自主推进「判决 → 选分支」，在岔路和「需人类判断处」raise 决策点，但**不自动 apply 图变更、不自动改假设生命周期**。再加一个 `agent run` CLI 让循环可跑。

## 编排者裁决（不可违反）

1. **纯 additive**：新建 `crates/agentflow-core/src/agent/mod.rs`（或 `agent.rs`）+ 在 `agent_ops_commands.rs` 加 `agent` 命令。**不得修改 H1–H6 任何已验收模块的现有函数**（只能调用其公开 API）。
2. **提议模式**：禁止调用 `apply_graph_patch`；禁止调用 `transition_hypothesis`（生命周期变更属用户决策）。图变更只允许走 `propose_graph_patch`/`propose_branch_patch`。
3. **不碰回退区间投影**（留 H7b）。
4. 不新增 crate 依赖；事件溯源；新增 event_type 仅 `agent.cycle_completed`；不新增表。
5. 质量门全绿：`clippy -D warnings` + `cargo test`。**基线 core 151 / cli 34 / schemas 3，不得破坏。**
6. 风格对齐现有模块；`#[cfg(test)] mod tests` 覆盖 ≥80%。

## 核心设计：自我节制的循环

```rust
pub enum CycleOutcome { HandedOff, Advanced, Idle }

pub struct CycleReport {
    pub checkpoint_id: String,
    pub provisional_verdicts: Vec<String>,    // 落库了 Provisional 判决的 hypothesis_id
    pub strong_candidates: Vec<String>,       // 预览出强判决、但因需 gate 而改为 raise 的 hypothesis_id
    pub raised_decisions: Vec<DecisionPoint>, // 本轮 raise 的决策点
    pub branch_proposals: Vec<BranchDecision>,// 记录的分支提议（未 apply）
    pub outcome: CycleOutcome,
}

impl ProjectStore {
    pub fn run_cycle(&self) -> Result<CycleReport, StorageError>;
}
```

`run_cycle` 步骤（确定性、可测）：

1. **checkpoint**：`create_checkpoint("agent_cycle")`，记下 id。
2. **判决阶段**：对每个 `list_hypotheses` 的假设，取 `evidence_for`：
   - 用 `RuleBasedEngine` **预览**判决（直接 `ArgumentEngine::render(...)`，纯函数，不落库）。
   - 若预览为 `Inconclusive(Provisional)` → 调 `render_verdict(id, &RuleBasedEngine, None)` 落库（Provisional 不需 gate），记入 `provisional_verdicts`。
   - 若预览为**强判决**（Affirmed/Refuted/Fundamental）→ **不落库**，改为 `raise_decision_point`：
     - kind = `DecisionKind::DeepenOrStop`
     - digest = 形如「假设 <id> 的证据预览为 <verdict>；需人类补防自欺 gate 后才能定论」（A3：必带凭证）
     - options = [`确认并补 gate`（推荐）, `继续收集证据`, `放弃该假设`]，recommendation = 0
     - 记入 `strong_candidates` 与 `raised_decisions`。
3. **选分支阶段**：`select_branches(&RuleBasedSelector, &BranchPolicy{ explore_enabled:false })`：
   - 对每个 `BranchAction::Abandon` 决策 → `raise_decision_point`（kind `GoalMutation`，停止分支属用户决策；options=[放弃/保留/再查]，推荐放弃），记入 `raised_decisions`。
   - 对 `Deepen`/`Spawn` 决策 → 记入 `branch_proposals`（**仅记录提议，不 apply、不 propose_branch_patch**，因为缺 ProposedStep 工具上下文）。`Hold` 跳过。
4. **结局判定**：
   - 有任何 `raised_decisions` → `outcome = HandedOff`。
   - 否则若有 `provisional_verdicts` 或 `branch_proposals` → `Advanced`。
   - 否则（无假设/无证据/无动作）→ `Idle`。
5. append `agent.cycle_completed`（payload 记 checkpoint_id + 各计数 + outcome）；返回 `CycleReport`。

> 设计意图：循环自主跑判决与选分支，但一旦预览出**强判决**或遇到**放弃分支**，就自然停下交人类（A2 + 防自欺）。这把「自动推进」与「该停就停」统一在一个确定性循环里，且全程提议、零不可逆写入。

## CLI：`agent run`

在 `agent_ops_commands.rs` 加：
- `agent run [--json] [--path <p>]` → `run_cycle()`，人类输出汇总（checkpoint、各计数、raise 的决策点摘要、outcome）；`--json` 走 `CycleReport::to_json`（新增 additive to_json）。
- lib.rs 加 `agent` dispatch 分支 + usage 文本（机械改动，不碰现有 handler）。

## 验收标准（Claude 审核逐条核对）

- [ ] `clippy -D warnings` 无警告；`cargo test` 全绿，core/cli 较基线净增。
- [ ] 无新依赖；新 event_type 仅 `agent.cycle_completed`；无新表。
- [ ] grep 确认 `agent` 模块**不含** `apply_graph_patch` / `transition_hypothesis`。
- [ ] 未修改 H1–H6 已验收模块的现有函数（`git diff` 核对：那些文件要么零改动，要么仅 additive 如 `CycleReport` 依赖的 to_json）。
- [ ] 行为测试：①只有弱证据 → 全 Provisional 落库 + outcome=Advanced；②强证据（observed 支持达 margin）→ 预览强判决 → 不落库、raise 决策点、outcome=HandedOff；③Refuted 候选触发 Abandon 决策点；④空项目 → Idle。
- [ ] `agent run` CLI happy-path + `--json` 测试；现有 34 cli 测试未改。

## 不在本里程碑（明确排除）

自动 apply 图变更（→ H7b）、回退区间接入域投影（→ H7b）、自主 forage 拉取（需检索工具）、真随机 ε、富 Goal 实体与多目标编排。
