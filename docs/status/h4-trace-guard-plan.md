# H4 实现简报：轨迹安全垫（checkpoint + 漂移检测 + 回退记录）

Status: Implemented + verified (2026-05-31)
Date: 2026-05-31
Owner(orchestrator): Claude · Executor: Codex
Spec source: [`agentflow-agent-control-layer-design.md`](../agentflow-agent-control-layer-design.md) §7 / §9-H4
Depends on: H1 / H2 / H3（已验收）

## 验收记录（Claude 独立复验 2026-05-31）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **135**（基线 129，+6）/ cli 22 / schemas 3 全绿。
- ✅ 仅改 `trace_guard.rs` + `lib.rs`；Cargo 零变更；无新表；event_type 精确 `trace.checkpoint_created` / `trace.reverted`。
- ✅ **不物删**：`revert_to` 只追加 `trace.reverted`，测试断言回退后 `events = before+1`；无 `DELETE`。
- ✅ 不碰域状态：无 `apply_graph_patch` / `transition_hypothesis`。
- ✅ `create_checkpoint` 在 append 前捕获 horizon（无事件为 None）。
- ✅ `detect_drift` 按 horizon 后自治事件计数；阈值 `DRIFT_SURFACE_THRESHOLD=5`；自治事件集合 = 5 个真实 event_type。
- ✅ `reverted_event_ids` 汇总去重，供 H7 接入域投影。

结论：合并就绪。H7 仍需让域投影尊重 `reverted_event_ids()`（已在排除项登记）。

## 目标

实现宪法 A4 的「可见 + 可回退」安全垫：checkpoint、累积漂移检测、回退记录。让默认自治可审计、可倒带。**纯 `agentflow-core` 库 + 测试；不做 CLI、不与主循环集成。**

## 编排者的架构裁决（不可违反，写在最前）

事件存储是 append-only。本里程碑：
- **完整实现** `create_checkpoint` + `detect_drift`（读侧 + 标记事件，完全可测）。
- `revert_to` **只做回退记录 + 回退区间查询**：append 一条 `trace.reverted` 记录目标 checkpoint 与被回退的事件 id 集合，并提供 `reverted_event_ids()` 查询。
- **禁止**在本步改写 H1–H3 已验收模块的投影去「honor 回退区间」，**禁止**物理删除任何事件。让各域投影真正尊重回退区间的接线，连同「真正发生自主 apply」一起放到 H7（H2 的自动应用本就推迟到 H7，故在 H7 之前不存在需要物理回滚的自主写入）。
- **禁止**调用 `apply_graph_patch` / `transition_hypothesis` 等写域状态的方法。

## 硬约束（与 H1–H3 相同）

1. 不新增任何 crate 依赖（禁 serde/rand）；JSON 手写复用同款私有助手。
2. 事件溯源：写走 `append_event`，读走 events 投影。新增 event_type **仅** `trace.checkpoint_created` / `trace.reverted`。不新增表。
3. `StorageError` 校验风格仿 `research.rs`。
4. 每模块 `#[cfg(test)] mod tests`，覆盖 ≥80%。
5. 质量门全绿：`clippy -D warnings` + `cargo test`。**基线 core 129 / cli 22 / schemas 3，不得破坏。**
6. `unsafe_code = forbid`；时间戳 `now_unix_seconds()`。

## 交付物

### `crates/agentflow-core/src/trace_guard.rs`（新建）

```rust
pub struct Checkpoint {
    pub id: String,            // = trace.checkpoint_created 事件 id
    pub horizon_event_id: Option<String>, // 创建时刻已存在的最后一条事件 id（无事件则 None）
    pub label: String,
    pub created_at: i64,
}

pub struct DriftReport {
    pub from_checkpoint: String,     // checkpoint id
    pub net_goal_delta: String,      // 描述串：自 checkpoint 起各类自治动作的计数摘要
    pub autonomous_steps: u32,       // 自 checkpoint 起的自治动作事件数
    pub should_surface: bool,        // 累积漂移是否已大到该主动交接
}
```

`impl ProjectStore`：

- `create_checkpoint(&self, label: &str) -> Result<Checkpoint, StorageError>`
  - 校验 label 非空；先查当前最后一条事件 id（`SELECT id FROM events ORDER BY created_at DESC, id DESC LIMIT 1`，无则 None）作为 `horizon_event_id`；append `trace.checkpoint_created`（payload 含 label + horizon_event_id）；返回。
- `list_checkpoints(&self) -> Result<Vec<Checkpoint>, StorageError>`（投影，created_at 升序）。
- `inspect_checkpoint(&self, id: &str) -> Result<Checkpoint, StorageError>`（NotFound 仿 research）。
- `detect_drift(&self, checkpoint_id: &str) -> Result<DriftReport, StorageError>`
  - 取 checkpoint；统计 `horizon_event_id` 之后（按 created_at,id 排序的严格之后；horizon 为 None 则全部）的「自治动作」事件。
  - **自治动作事件类型集合**（常量）：`hypothesis.transitioned`、`argument.verdict_rendered`、`argument.evidence_linked`、`graph_patch_proposed`（若现有类型名不同，Codex 按实际 graph_patch 事件类型填），`handoff.decision_point_raised`。
  - `autonomous_steps` = 上述事件计数；`should_surface = autonomous_steps >= DRIFT_SURFACE_THRESHOLD`（常量，设 5）；`net_goal_delta` = 按类型计数的确定性描述串。
- `revert_to(&self, checkpoint_id: &str) -> Result<RevertRecord, StorageError>`
  - 校验 checkpoint 存在；收集其 `horizon_event_id` 之后的全部事件 id；append `trace.reverted`（payload 含 `checkpoint_id` + 被回退事件 id 列表或其计数）；返回 `RevertRecord { id, checkpoint_id, reverted_event_ids: Vec<String>, created_at }`。
  - **不**物理删除事件、**不**改任何域投影。
- `reverted_event_ids(&self) -> Result<Vec<String>, StorageError>`
  - 投影所有 `trace.reverted` 事件，汇总被标记为已回退的事件 id（去重）。供 H7 将来让域投影尊重回退区间使用。

> 说明：`detect_drift` 与 `revert_to` 的「自治动作集合」请用实际存在的 event_type 字符串；Codex 先 grep 现有 graph_patch 提议事件的真实 event_type 再填，避免臆造。

## 验收标准（Claude 审核逐条核对）

- [ ] clippy `-D warnings` 无警告；`cargo test` core 较 129 净增，全绿。
- [ ] 无新依赖、无新表；新 event_type 仅 `trace.checkpoint_created` / `trace.reverted`。
- [ ] `create_checkpoint` 正确捕获 horizon（含「无事件时为 None」「之后追加事件不改已存 checkpoint 的 horizon」两测试）。
- [ ] `detect_drift`：horizon 之后的自治动作计数正确；`should_surface` 阈值正确（边界测试）。
- [ ] `revert_to`：记录正确的被回退事件 id 集合；**不删事件**（回退后 events 总数只增不减，有测试断言）。
- [ ] `reverted_event_ids` 汇总去重正确。
- [ ] grep 确认 `trace_guard.rs` 不含 `apply_graph_patch` / `transition_hypothesis`，不含物理 `DELETE`。

## 不在本里程碑（明确排除）

让域投影尊重回退区间（→ H7）、物理回滚物化表（→ H7）、CLI、与主循环集成、真随机/自动漂移触发交接（→ H7）。
