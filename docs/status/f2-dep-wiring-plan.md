# F2 实现简报：依赖边自动接线 + apply 容错

Status: Implemented + verified (2026-06-02)
Date: 2026-06-02
Owner(orchestrator): Claude · Executor: Codex
Spec source: 待完善清单 #2（高优先）—— 自治步骤的 needs 推断，让自动加的步骤接进图
Depends on: T1（draft_step_for）、T2（enrich_branch_proposal）、H7b-2（apply 路径）—— 均已合并 main

## 验收记录（Claude 独立复验 2026-06-02）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **176**（基线 172，+4）/ cli 48 / schemas 3 全绿。
- ✅ 改动局限 `tool_select.rs`（+infer_step_needs）+ `agent.rs`（enrich 接 needs、apply 容错）；Cargo 零变更；无新表/event_type。
- ✅ **`draft_step_for` 完全未改**（不在 diff）；两处 `+needs: Vec::new()` 均在新测试 fixture 内。
- ✅ `infer_step_needs`：computed 工件→含生产步骤、imported→排除、占位/缺失→跳过、同源去重排序（2 测试）。
- ✅ `enrich_branch_proposal` 填 needs：computed 输入→含生产步骤，imported→空（测试覆盖）。
- ✅ apply 容错：`CycleReport.apply_failures`（additive）记录失败并继续，不中断整轮（测试覆盖）；**无失败时输出结构不变**（`apply_failures_field` 空串）。

结论：合并就绪。自治步骤现接入上游生产步骤；apply 失败不再中断整轮。跨 flow 的 needs 校验/自动补缺失步骤留后续。

## 目标

1. 为草拟的 `ProposedStep` **推断 `needs`**：对每个输入工件回溯其 `source_step_id`（生产它的步骤），填进 needs，让提议/自治应用的步骤连接到上游，而非孤立。
2. **配套容错**：needs 非空后，某条 graph-patch apply 可能因 needs 引用了不在目标 flow 的步骤而失败——把 apply 失败改为**记录并继续**（非致命），避免一条坏提议中断整轮 `agent run --apply`。

## 编排者裁决（约束）

1. 改动局限 `crates/agentflow-core/src/tool_select.rs`（新增 `infer_step_needs`，**不改 `draft_step_for`**）+ `agent.rs`（enrich 接 needs、apply 容错）。不改其它已验收模块逻辑。
2. **`draft_step_for` 保持原样**（T1 测试不动）：needs 推断作为独立 helper，在 `enrich_branch_proposal` 里叠加。
3. 默认提议模式语义不变（needs 填充只是让 drafted_step 更完整，不触发任何写）。
4. 不新增依赖/表/event_type。
5. 容错只针对 apply 路径（`--apply`）；不改变「强判决/Abandon 仍交接」「默认关零变化」等既有契约。

## 交付物

### 1. `tool_select.rs`：`infer_step_needs`（新增，additive）
```rust
impl ProjectStore {
    /// 对 step.inputs 的每个 (_, artifact_id)：inspect_artifact 取 source_step_id；
    /// 收集 Some 的 step_id，去重、排序返回。容错：artifact 不存在/占位符 → 跳过。
    pub fn infer_step_needs(&self, step: &crate::branch::ProposedStep)
        -> Result<Vec<String>, StorageError>;
}
```
- imported 根工件（`source_step_id == None`）→ 不产生依赖。
- 占位符输入（如 `artifact_REPLACE_*`）或 NotFound → 跳过，不报错。

### 2. `agent.rs`：`enrich_branch_proposal` 接 needs
- `draft_step_for(...)` 得到 step 后，`needs = infer_step_needs(&step)`，用填好的 needs **重建** `ProposedStep`（字段 pub，构造新值，保持不可变风格）。
- 这样 `EnrichedProposal.drafted_step` 带真实上游依赖；apply 路径复用同一 step。

### 3. `agent.rs`：apply 路径容错（仅 `--apply` 分支）
- 当前 `propose_branch_patch(...)?` / `approve_graph_patch(...)?` / `apply_graph_patch(...)?` 任一失败会 `?` 中断整轮。
- 改为：捕获该提议的 apply 错误 → 记入报告（如 `CycleReport` 新增 `apply_failures: Vec<ApplyFailure { hypothesis_id, reason }>`，additive）→ **继续处理后续提议**，不中断 run。
- 其它路径（生命周期 transition、raise）不变。

### `agent run` 输出
- `--json` / 人类输出 additive 增加 `apply_failures`（若有）。无失败时不改变现有输出结构（保持回归）。

## 验收标准（Claude 审核逐条核对）

- [ ] `clippy -D warnings` 无警告；`cargo test` 全绿，净增测试。
- [ ] 无新依赖/表/event_type；`draft_step_for` 未改（T1 测试不动）；其它模块逻辑未改。
- [ ] `infer_step_needs` 测试：①computed 工件（有 source_step_id）→ needs 含该步骤；②imported 工件 → 不含；③占位符/缺失工件 → 跳过不报错；④多输入同源去重。
- [ ] `enrich_branch_proposal` 测试：输入映射到 computed 工件时 `drafted_step.needs` 含生产步骤；映射到 imported 工件时 needs 为空。
- [ ] apply 容错测试：构造一个会让 apply 失败的提议（如 needs 引用不存在的 step）→ 该提议记入 `apply_failures` 且 `agent run --apply` 不中断、后续仍处理。
- [ ] 回归：默认（无 --apply）与无 apply_failures 时输出结构不变；现有 agent/cli 测试通过（必要时仅更新 agent.rs 内断言，语义不变）。

## 不在本里程碑（明确排除）

跨 flow 的 needs 校验/重写、自动创建缺失的上游步骤、add_edge 的方向/类型语义扩展、把 needs 推断塞进 draft_step_for（保持其纯净）。
