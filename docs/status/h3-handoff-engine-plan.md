# H3 实现简报：交接引擎（决策点 + 刹车策略 + 决策/体力活分类）

Status: Implemented + verified (2026-05-31)
Date: 2026-05-31
Owner(orchestrator): Claude · Executor: Codex
Spec source: [`agentflow-agent-control-layer-design.md`](../agentflow-agent-control-layer-design.md) §6 / §9-H3
Depends on: H1 / H2（已验收）

## 验收记录（Claude 独立复验 2026-05-31）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **129**（基线 116，+13）/ cli 22 / schemas 3 全绿。
- ✅ 仅改 `handoff.rs` + `lib.rs`；4 个 Cargo.toml 零变更；无新表；event_type 精确 `handoff.decision_point_raised` / `handoff.user_resolved`。
- ✅ **A3 不变量**：`validate_raise_input` 强制 digest 非空 + options 非空 + recommendation 合法下标。
- ✅ `DefaultPolicy::assess` 优先级 goal→premise→budget→（贵/不可逆/分叉）→Proceed，逐条一致。
- ✅ `resolve_decision_point` 拒重复解决、拒越界 chosen_index。
- ✅ `classify`：Decision ⟺ consequential ∧ user_cares。
- ✅ `handoff.rs` 不含 `transition_hypothesis` / `apply_graph_patch`。

结论：合并就绪。

## 目标

实现控制宪法 A1–A3 的可执行化：**何时把球交回用户**。核心库 `handoff.rs`：刹车触发策略（挂错误代价而非自信度）、决策点的提出/列出/解决、决策 vs 体力活分类器。**纯 `agentflow-core` 库 + 测试；本里程碑不做 CLI**（CLI 统一在后续 CLI 里程碑补）。

## 硬约束（与 H1/H2 相同）

1. 不新增任何 crate 依赖（禁 serde/rand）；JSON 手写复用同款私有助手。
2. 事件溯源：写走 `append_event`，读走 events 投影。新增 event_type **仅** `handoff.decision_point_raised` / `handoff.user_resolved`。不新增数据库表。
3. `StorageError` 校验风格仿 `research.rs`。
4. 每模块 `#[cfg(test)] mod tests`，覆盖 ≥80%：刹车表每条分支、A3 校验失败、分类器、提出/解决往返、重复解决被拒。
5. 质量门全绿：`clippy -D warnings` + `cargo test`。**基线 core 116 / cli 22 / schemas 3，不得破坏。**
6. `unsafe_code = forbid`；时间戳 `now_unix_seconds()`。

## A3 不变量（必须强制，违反即返工）

- **决策点必带凭证**：`digest`（已干过的活）非空，否则 `InvalidInput`——杜绝「裸问题上交 / 没尝试就上交」。
- **必带推荐**：`recommendation` 必须是 `options` 的合法下标。
- **选项非空**：Raise 出的决策点 `options` 不得为空（顾问而非甩手掌柜）。

## 交付物

### `crates/agentflow-core/src/handoff.rs`（新建）

```rust
pub enum Cost { Cheap, Moderate, Expensive }          // as_str/parse
pub enum Risk { Low, Medium, High }                   // as_str/parse
pub enum DecisionKind {                               // as_str/parse
    DeepenOrStop, PremiseChallenged, BudgetThreshold, GoalMutation,
}
pub enum TaskClass { Labor, Decision }

pub struct HandoffOption {
    pub label: String,
    pub direction: String,
    pub cost: Cost,
    pub risk: Risk,
    pub reversible: bool,
}

pub struct DecisionPoint {
    pub id: String,                 // = decision_point_raised 事件 id
    pub kind: DecisionKind,
    pub digest: String,             // A3：非空
    pub options: Vec<HandoffOption>,// A3：非空
    pub recommendation: usize,      // A3：< options.len()
    pub status: DecisionStatus,     // Pending / Resolved
    pub resolution: Option<Resolution>,
    pub created_at: i64,
}
pub enum DecisionStatus { Pending, Resolved }
pub struct Resolution { pub chosen_index: usize, pub note: String, pub resolved_at: i64 }

/// 刹车策略输入：描述「下一步动作的后果」，触发挂在后果上而非自信度上。
pub struct StepContext {
    pub cost: Cost,
    pub reversible: bool,
    pub equivalent_branches: bool,   // 多条等价分叉
    pub conflicts_user_premise: bool,
    pub mutates_goal: bool,
    pub near_budget: bool,
}

/// 刹车策略：返回 None=自治往前；Some(kind)=必须交接（用哪类决策点）。
pub trait InterventionPolicy {
    fn assess(&self, ctx: &StepContext) -> Option<DecisionKind>;
}
pub struct DefaultPolicy;
```

`DefaultPolicy::assess` 判定（**按此优先级顺序**，命中即返回）：
1. `ctx.mutates_goal` → `Some(GoalMutation)`
2. `ctx.conflicts_user_premise` → `Some(PremiseChallenged)`
3. `ctx.near_budget` → `Some(BudgetThreshold)`
4. `matches!(ctx.cost, Expensive) || !ctx.reversible || ctx.equivalent_branches` → `Some(DeepenOrStop)`
5. 否则（便宜 && 可逆 && 单一路径 && 不触发上面）→ `None`（Proceed，不问自己走）

> 注：优先级把「动到目标 / 顶用户前提」排在「贵/不可逆」之前，因为前两者更值钱、更该先交接。

分类器：
```rust
/// 此处不同的合理选择是否实质改变用户在乎的结果？是→Decision；否/用户不在乎→Labor。
pub fn classify(consequential: bool, user_cares: bool) -> TaskClass;
// Decision 当且仅当 consequential && user_cares；否则 Labor。
```

`impl ProjectStore`：
- `raise_decision_point(&self, kind: DecisionKind, digest: &str, options: Vec<HandoffOption>, recommendation: usize) -> Result<DecisionPoint, StorageError>`
  - 校验 A3 三条（digest 非空、options 非空、recommendation 合法下标），否则 `InvalidInput`；append `handoff.decision_point_raised`；返回 status=Pending。
- `list_decision_points(&self) -> Result<Vec<DecisionPoint>, StorageError>`
  - 投影：折叠 `decision_point_raised`（基态）+ `handoff.user_resolved`（按 payload `decision_point_id` 置 status=Resolved + resolution），created_at 升序。
- `pending_decision_points(&self) -> Result<Vec<DecisionPoint>, StorageError>`（仅 Pending）。
- `inspect_decision_point(&self, id: &str) -> Result<DecisionPoint, StorageError>`（NotFound 仿 research）。
- `resolve_decision_point(&self, id: &str, chosen_index: usize, note: &str) -> Result<DecisionPoint, StorageError>`
  - 校验决策点存在且 status=Pending（已解决再解决 → `InvalidInput`）；`chosen_index < options.len()` 否则 `InvalidInput`；append `handoff.user_resolved`（payload 含 `decision_point_id`/`chosen_index`/`note`）；返回解决后投影。

## 验收标准（Claude 审核逐条核对）

- [ ] clippy `-D warnings` 无警告；`cargo test` core 较 116 净增，全绿。
- [ ] 无新依赖、无新表；event_type 仅 `handoff.decision_point_raised` / `handoff.user_resolved`。
- [ ] 刹车表 5 条分支各有测试，且优先级顺序正确（mutates_goal 先于 cost）。
- [ ] A3 三条校验各有失败测试（空 digest / 空 options / 越界 recommendation 被拒）。
- [ ] `classify` 四种组合有测试。
- [ ] 提出→解决往返正确；重复解决被拒；解决非法 chosen_index 被拒。
- [ ] `resolve_decision_point` 不触碰 hypothesis / graph（grep 确认 handoff.rs 不含 `transition_hypothesis` / `apply_graph_patch`）。

## 不在本里程碑（明确排除）

CLI（→ 后续 CLI 里程碑）、与主循环集成（→ H7）、轨迹漂移联动（→ H4）、富 Goal 建模（→ H7，当前 classify 用显式布尔信号）。
