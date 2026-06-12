# AS14 实现简报：结果驱动的机制子假设 spawn（Robin 原则 C，自动建 proposed 子假设，确定性、有防爆护）

Status: Assigned to Codex（新分支 feat/mechanism-spawn，从 main 起，main 已含 AS7–AS13）
Owner: Claude(编排) · Codex(执行)
Spec source: 对照 Robin（Nature s41586-026-10652-y）"结果支持后追问机制"（Y-27632 有效 → 为何 → ROCK 通路 → 下一轮）
Depends on: AS1–AS13（已并入 main）

## 背景与门控裁决

当前 `BranchAction::Spawn`（affirmed 判决产生）与 `Deepen` 在 `run_cycle_inner`（`agent.rs:486`）**同一 match 臂、处理完全相同**——只对同一假设 draft 工具步骤，**从不派生机制子假设**。本里程碑补这一环：假设被 affirmed 时，自动派生一个**机制探究子假设**，把"X 是否成立"推进到"X 的机制是什么"。

**编排者门控裁决（已与用户确认）**：
- **自动建 `proposed` 子假设**（不走新 DecisionKind / 不改事件结构）。proposed 假设不自动影响任何东西，人可在 `hypothesis list` 里 review/abandon——human-in-the-loop 由 proposed 生命周期天然保证。
- **确定性模板生成机制问题，不动 LLM**（不新增 LLM trait/seam）。
- **严格防爆护**（见下），保证假设空间最多每个 affirmed 根假设派生 **1 个** 子假设、且**仅一次**、**不成链**。

## 编排者裁决（约束）

### 1. 在 Spawn 处理里加守卫式机制子假设创建

`crates/agentflow-core/src/agent.rs` `run_cycle_inner` 的 `BranchAction::Deepen { .. } | BranchAction::Spawn { .. }` 臂（约 486）顶部，新增**仅当 action 是 Spawn** 时执行的逻辑（其余 enrich/draft 逻辑保持不变，机制子假设是额外副作用）：

```rust
if matches!(&decision.action, BranchAction::Spawn { .. }) {
    if let Some(spawned) = self.maybe_spawn_mechanism_child(&decision.candidate)? {
        applied.push(AppliedAction::MechanismHypothesisSpawned {
            parent_id: decision.candidate.hypothesis_id.clone(),
            child_id: spawned.0,
            statement: spawned.1,
        });
    }
}
```

新增方法 `fn maybe_spawn_mechanism_child(&self, candidate: &BranchCandidate) -> Result<Option<(String, String)>, StorageError>`：

1. `parent = self.inspect_hypothesis(&candidate.hypothesis_id)?`。
2. **守卫 A（不成链）**：若 `parent.origin` 以 `MECHANISM_SPAWN_ORIGIN_PREFIX` 开头 → 返回 `None`（机制子假设不再派生孙子假设）。
3. **守卫 B（每父一次，跨 cycle 幂等）**：扫 `self.list_hypotheses()?`，若已存在 `origin == format!("{MECHANISM_SPAWN_ORIGIN_PREFIX}{parent_id}")` 的假设 → 返回 `None`。
4. **确定性模板**（通用，**无任何领域/基因/疾病常量**）生成 statement，例如：
   `format!("机制探究：哪些分子机制可解释「{}」？需要哪些可直接检验该机制的证据？", parent.statement.trim())`
5. `let child = self.record_hypothesis(HypothesisRequest { statement, origin: format!("{MECHANISM_SPAWN_ORIGIN_PREFIX}{}", parent.id), related_goal_id: parent.related_goal_id.clone() })?;`
6. 返回 `Some((child.id, child.statement))`。

新增常量：`const MECHANISM_SPAWN_ORIGIN_PREFIX: &str = "mechanism-spawn:from:";`（通用 slug）。

### 2. AppliedAction 新增变体（additive，向后兼容）

`crates/agentflow-core/src/agent.rs` 的 `pub enum AppliedAction`（约 222）新增：
```rust
MechanismHypothesisSpawned {
    parent_id: String,
    child_id: String,
    statement: String,
},
```
这是 serde 外部 tag 枚举的**新增变体**：既有序列化 payload 仍可反序列化（旧 payload 不含该变体），与 `LifecycleTransition` 等记录方式一致。不改其它事件/payload 字段结构、不改 `DecisionKind`、不改 `HypothesisRequest`/`Hypothesis` schema（复用 `origin` 字段编码父链接）。

### 3. 不变量与约束

- 仅 affirmed 触发（`action_for` 中 Spawn 只由 affirmed 判决产生；本逻辑只在 Spawn 臂执行）。
- 防爆：守卫 A + B 保证每个根假设最多 1 个机制子、仅一次、不成链；子假设是 `proposed`，本 cycle 不会被重复处理（decisions 本 cycle 已选定；下 cycle 才作为 proposed 进入正常生命周期）。
- 确定性：模板固定；无 LLM/网络。`argument.rs` 不动，仍 0 处 LLM/网络。
- 通用：核心**无任何具体基因/疾病/PMID 常量**；模板只包裹父 statement。
- 无新依赖；core 仍 0 LLM/网络依赖。

## 测试（离线，确定性）

`agent.rs` 新增单测：
- **派生**：一个会被判 affirmed 的假设跑 `run_cycle` → 新增一个 `proposed` 机制子假设，其 `origin == "mechanism-spawn:from:<parent_id>"`、`related_goal_id == parent.goal`、statement 含父 statement 与"机制探究"；`report.applied` 含 `MechanismHypothesisSpawned`。
- **守卫 B（幂等）**：同项目连跑两个 cycle → 只创建一个子假设（第二轮不重复）。
- **守卫 A（不成链）**：让机制子假设本身也 affirmed → 不派生孙子假设。
- **非 affirmed 不派生**：provisional/Deepen 假设 → 无机制子假设。
- 既有断言 `applied`/hypothesis 数量的测试按新行为更新。
- core 测试数预期 +3~4。

## 验收标准（Claude 复核 + live）

- [ ] fmt / clippy / core / cli / `scripts/acceptance-v1.sh` 全绿；`argument.rs` 仍 0 处 LLM/网络。
- [ ] 单测证明派生、幂等(守卫 B)、不成链(守卫 A)、非 affirmed 不派生。
- [ ] AppliedAction 新增变体向后兼容（既有 payload 反序列化测试保持绿）。
- [ ] **live（编排者，纯 `agent run`，不干预）**：对一个已 affirmed 的假设，观察自动派生出一个 proposed 机制子假设，下一 cycle 它进入正常生命周期；无重复、无链式爆炸。
- [ ] 核心无单次任务常量；无新依赖；core 测试数不减少。

## 不在本里程碑

- 不用 LLM 生成机制问题（模板优先；LLM 智能化可作未来精化）。
- 不做 DecisionKind/人工确认门（已选自动 proposed 模型）。
- 不做机制子假设的自动实验编排（它走既有 proposed→工具匹配/合成 链路）。
- 不改 `examples/tools/*`。
