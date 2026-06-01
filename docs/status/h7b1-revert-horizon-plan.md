# H7b-1 实现简报：回退区间接入域投影

Status: Implemented + verified (2026-06-01)
Date: 2026-06-01
Owner(orchestrator): Claude · Executor: Codex
Spec source: [`agentflow-agent-control-layer-design.md`](../agentflow-agent-control-layer-design.md) §7 / §9-H7（H4 排除项「让域投影尊重回退区间」的兑现）
Depends on: H1–H7a（已验收、已合并 main）

## 验收记录（Claude 独立复验 2026-06-01）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **161**（基线 155，+6）/ cli 40 / schemas 3 全绿。
- ✅ **回归零破坏**：198 个既有测试未改且通过。
- ✅ Cargo 零变更；无新表；无新 event_type。
- ✅ 改 5 个生产投影（hypothesis list/inspect、argument evidence_for/latest_verdict_for、handoff decision list/pending/inspect、forage list/inspect）+ `reverted_event_id_set` 助手，折叠时跳过已回退事件。
- ✅ 未引入生产 auto-apply / 自主 transition（新增 `transition_hypothesis` 调用均在 `#[cfg(test)]` 内造回退场景）。
- ✅ **端到端冒烟**：checkpoint → 建假设+证据 → `revert_to` → `list_hypotheses` 空、`inspect` NotFound、`evidence_for` 空；事件**未物理删除**（append-only 完整）。

结论：合并就绪。`trace revert` 从「只记录」升级为「真回滚」，A4 安全垫生效。**H7b-2 自动 apply 开闸仍待用户显式授权。**

## 目标

让 `trace revert` **真正生效**：所有域投影在重建状态时**跳过已被回退的事件**（id ∈ `reverted_event_ids()`）。这是 H7b 的**安全前提**（auto-apply 开闸前，回退必须能真正回滚状态），本身也让 `trace revert` 从「只记录」变为「真回滚」。

> 注意：本里程碑**不**做 auto-apply 开闸（那是 H7b-2，需用户显式授权）。本步只动「读」侧投影，不引入任何自主写入。

## 编排者裁决（这是唯一会动已验收代码的里程碑，约束从严）

1. **行为对未回退场景必须零变化**：当没有任何 `trace.reverted` 事件时，所有投影结果与现在**逐字节相同**。现有 198 个测试**必须全部不改且通过**（这是首要回归保护）。
2. 不新增依赖；不新增表；不新增 event_type。
3. 只改「读」侧投影；禁止改任何「写」方法的语义；禁止 auto-apply / 自主 transition。
4. 质量门全绿：`clippy -D warnings` + `cargo test`。基线 core 155 / cli 40 / schemas 3。

## 交付物

### 1. 共享助手（`trace_guard.rs` 或 `storage`）

```rust
impl ProjectStore {
    /// 已被回退的事件 id 集合（复用现有 reverted_event_ids，去重为集合）。
    pub fn reverted_event_id_set(&self) -> Result<std::collections::HashSet<String>, StorageError>;
}
```

### 2. 在下列**生产投影**中跳过 id ∈ reverted 集合的事件

逐一改，语义为「折叠事件时忽略已回退事件」：

- `hypothesis.rs`：`list_hypotheses`、`inspect_hypothesis`
  - 若某假设的 `hypothesis.created` 被回退 → 该假设不出现（inspect → NotFound）。
  - 若仅某次 `hypothesis.transitioned` 被回退 → 跳过该跃迁（状态回到上一个未回退状态）。
- `argument.rs`：`evidence_for`、`latest_verdict_for`（及任何证据/判决列举）
  - 被回退的 `argument.evidence_linked` / `argument.verdict_rendered` 不计入。
- `handoff.rs`：`list_decision_points`、`pending_decision_points`、`inspect_decision_point`
  - 被回退的 `handoff.decision_point_raised` → 该决策点消失；被回退的 `handoff.user_resolved` → 视为未解决。
- `forage.rs`：`list_forage_observations`、`inspect_forage_observation`
  - 被回退的 `forage.observation_recorded` → 不出现。

> 实现建议：每个投影开头取一次 `reverted_event_id_set()`，在折叠循环里 `if reverted.contains(&event_id) { continue; }`。保持各模块现有手写 JSON / 查询风格，不重构。

### 3. 不在本步处理（明确）

- `trace_guard::detect_drift`（漂移度量按原始活动计数即可，保持现状）。
- `agent::run_cycle`：它通过上述域投影间接获得过滤效果，不需单独改。
- auto-apply 图变更、自主 transition（→ H7b-2，需显式授权）。

## 验收标准（Claude 审核逐条核对）

- [ ] **回归**：现有 198 测试零改动且通过（首要）。
- [ ] `clippy -D warnings` 无警告；新增测试覆盖 ≥80% 新分支。
- [ ] 无新依赖/表/event_type。
- [ ] 端到端回退测试：`create_checkpoint` →（之后）创建假设 H + 证据 + 判决 → `revert_to(checkpoint)` → `list_hypotheses` 不含 H、`evidence_for(H)` 空、`latest_verdict_for(H)` 为 None、`inspect_hypothesis(H)` NotFound。
- [ ] 部分回退测试：跃迁被回退后状态回到前一状态。
- [ ] 决策点/forage 观察 的回退各 1 测试。
- [ ] 未回退场景投影结果不变（对照测试）。

## 不在本里程碑（明确排除）

H7b-2 自动 apply 开闸（需用户显式授权）、detect_drift 的回退感知、性能物化优化。
