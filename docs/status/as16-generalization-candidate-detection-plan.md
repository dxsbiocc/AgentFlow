# AS16 实现简报：能力指纹 + 可泛化候选检测（Tool Evolution 第二阶段，只看不改，确定性）

Status: Assigned to Codex（新分支 feat/generalization-candidate-detection，从 main 起，main 已含 AS15 + RFC）
Owner: Claude(编排) · Codex(执行)
Spec source: docs/design/tool-evolution-engine-design.md §4① / §11.2 / §12（AS16，由 AS15 mismatch 信号驱动）
Depends on: AS1–AS15（已并入 main，含 AS15 output-domain-mismatch 信号）

## 背景与范围

AS15 让 agent 检出"工具输出领域≠假设领域"并记 `output-domain-mismatch:` apply_failure。AS16 把这个信号**升级为可泛化候选**：产出错领域结果的工具，本质是"同能力、cohort 锁定"的近邻——它正是泛化的对象。AS16 **只检测、只 surface，不改任何工具、不合成、不新增 DecisionKind/持久事件**（RFC §11.2 tier-1 的 I/O 签名硬门用确定性实现；LLM 同领域动作确认留到 AS17 真正分组泛化时）。

## 编排者裁决（约束）

### 1. 能力指纹（确定性，结构性 I/O 签名）

`crates/agentflow-core/src/agent.rs` 新增：
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityFingerprint {
    pub output_types: Vec<String>,         // 排序去重
    pub required_input_types: Vec<String>, // 排序去重
}
```
从 `inspect_tool(tool_ref)?.spec_json`(经既有解析)取 output 类型与 required input 类型，排序去重构造。纯结构、确定性、**无任何领域常量**。

### 2. 可泛化候选记录 + CycleReport 字段（additive）

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneralizationCandidate {
    pub tool_ref: String,
    pub hypothesis_id: String,
    pub fingerprint: CapabilityFingerprint,
    pub io_compatible_peers: Vec<String>, // 同指纹的其它已注册工具(不含自身),排序
    pub evidence: String,                 // 如 "output-domain-mismatch: ..."
}
```
`CycleReport`(agent.rs:322)新增 `#[serde(default)] pub generalization_candidates: Vec<GeneralizationCandidate>`（与现有 `applied`/`apply_failures` 同样 serde-default，向后兼容）。

### 3. 检测点：AS15 mismatch 分支

在 `raise_stance_assessment_for_observation` 的 `Some(false)` 分支（AS15 写 apply_failure 处）**之后**，追加可泛化候选检测：
1. **解析产出该 observation 的 tool_ref**：优先 `auto_synthesized_tool_for_observation`(synth 工具)；否则经 `observation.flow_id` + `observation.step_id` 查 flow 步骤定义取其 `tool_ref`(用既有 flow inspection API)。**容错**：解析不出就跳过候选（不 panic、不阻断）。
2. `fingerprint = capability_fingerprint(self, &tool_ref)?`（容错：inspect 失败则跳过）。
3. **I/O 兼容 peers**（确定性硬门）：扫 `list_tools()`，对每个 inspect+指纹，收集**与候选指纹相等、且 tool_ref≠自身**的，排序。
4. push `GeneralizationCandidate { tool_ref, hypothesis_id, fingerprint, io_compatible_peers, evidence }` 到一个 `&mut Vec<GeneralizationCandidate>`（从 `run_cycle_inner` 沿 `auto_run_applied_step` 透传进来，与 `apply_failures` 同样方式）。
- 函数签名补 `generalization_candidates: &mut Vec<GeneralizationCandidate>`；run_cycle_inner 收集后塞进 CycleReport。

### 4. CLI 渲染（非阻塞 notice）

`crates/agentflow-cli` agent run 文本输出新增一段（json 模式天然带 `generalization_candidates`）：
`🔁 可泛化候选: <tool_ref>（I/O 同签名 peers: …）— 因 output-domain-mismatch；候选：参数化领域(cohort)以通用`。

### 5. 不变量

- **只看不改**：不改任何工具、不合成、不晋升、不删除；无 tool/spec 写操作。
- 不碰 `argument.rs`(判决确定性 0 LLM/网络)；本里程碑**不引入 LLM**(指纹/peers 全确定性；LLM 同领域动作确认是 AS17 的事)。
- 不新增 DecisionKind、不新增持久事件(候选可由已持久的 mismatch apply_failure + 工具注册表在 AS17 重新推导；AS16 只在本 cycle report 内 surface)。
- additive serde 字段，向后兼容；核心无任何具体基因/疾病/study 常量。
- 无新依赖。

## 测试（离线，确定性）

`agent.rs`：
- **mismatch→候选**：stub grounding `Some(false)`，跑出 observation 的 cycle → `report.generalization_candidates` 含一条，`tool_ref` 正确、`fingerprint` 为该工具 I/O 签名、`evidence` 含 `output-domain-mismatch`。
- **peers 检测**：注册两个同 I/O 签名工具，mismatch 命中其一 → 候选的 `io_compatible_peers` 含另一个。
- **无 mismatch→空**：grounding `Some(true)`/Noop → `generalization_candidates` 为空。
- **指纹确定性**：同一工具多次 `capability_fingerprint` 相等；输入顺序不影响(排序)。
- **容错**：tool_ref 解析不出 / inspect 失败 → 不 panic、该 observation 不产候选。
core 测试数预期 +3~4。

## 验收标准（Claude 复核）

- [ ] fmt / clippy / core / cli / `scripts/acceptance-v1.sh` 全绿；`argument.rs` 仍 0 处 LLM/网络。
- [ ] 单测证明 mismatch→候选(含正确 fingerprint/peers)、无 mismatch→空、指纹确定性、容错。
- [ ] CycleReport 新字段 serde 向后兼容（既有 payload 反序列化保持）。
- [ ] **只看不改**：无任何工具/spec 写操作；无新 DecisionKind/持久事件；不引入 LLM；核心无领域常量。
- [ ] core 测试数不减少；无新依赖。

## 不在本里程碑

- 不做变异点识别 / 重构式泛化合成 / 验证门 / 扬弃谱系——AS17/18。
- 不做 LLM 同领域动作确认（AS17 分组泛化时再用）。
- 不做跨任务使用复发计数（本阶段只走 AS15 mismatch 触发；计数路径留待后续）。
- 不改 `examples/tools/*`。
