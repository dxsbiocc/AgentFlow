# AS15 实现简报：输出领域校验（Tool Evolution RFC 首阶段，止"蒙头用"之血）

Status: Assigned to Codex（新分支 feat/output-domain-validation，从 main 起）
Owner: Claude(编排) · Codex(执行)
Spec source: docs/design/tool-evolution-engine-design.md §5 / §11 / §12（首阶段 B）
Depends on: AS1–AS14（已并入 main）

## 背景

闭环验证(SPP1/LUAD)发现:`tcga/survival_assoc` 把队列硬编码成 LIHC(肝),对 LUAD(肺)假设静默跑出**肝癌**结果,系统**没读自己的输出、没发现 lihc≠LUAD,蒙头当成发现交了出去**(只被动标"参数未确认")。

AS15 = 让 agent **读自己的真实输出**,校验其领域是否真针对假设;不匹配则**拒绝当证据**,而不是走"看似有效"的 stance 交接。这是 Tool Evolution 引擎的**触发器**(矛盾的暴露);进化本身(AS16-18)不在本里程碑。

## 编排者裁决（约束）

### 1. 新增 LLM seam `OutputGroundingScorer`（Noop 默认，无回归）

`crates/agentflow-core/src/agent.rs`,仿 `RelevanceScorer`(agent.rs:95-118)新增:
```rust
pub trait OutputGroundingScorer {
    /// Whether a produced finding actually addresses the hypothesis's domain/claim
    /// (e.g. the finding's cohort/disease matches the hypothesis), not merely the topic.
    /// Returns None when undecidable (e.g. no LLM configured).
    fn grounds_hypothesis(&self, hypothesis_statement: &str, finding_text: &str) -> Option<bool>;
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NoopOutputGroundingScorer;
impl OutputGroundingScorer for NoopOutputGroundingScorer {
    fn grounds_hypothesis(&self, _h: &str, _f: &str) -> Option<bool> { None }
}
```

### 2. 线接：穿到 stance 触发点（向后兼容，加 grounded 变体）

- `run_cycle_inner`(agent.rs:421)新增参数 `grounding: &dyn OutputGroundingScorer`，沿调用链穿到 `auto_run_applied_step`(1267) → `raise_stance_assessment_for_observation`(1329)。
- **向后兼容**:既有 `run_cycle_with_synth`/`run_cycle_with_scorer`/... 签名**不变**,内部以 `&NoopOutputGroundingScorer` 调 `run_cycle_inner`。**新增**一个 public 入口 `run_cycle_with_synth_grounded(config, inferer, scorer, synthesizer, grounding)` 透传真实 grounding。这样既有调用方/测试零改动。

### 3. 校验点逻辑（`raise_stance_assessment_for_observation`）

在该函数取得 `observation` 与 `hypothesis` 之后、raise StanceAssessment **之前**插入:
1. 构造 `finding_text` = `observation.summary` + **容错读取产物正文**:若 `observation.artifact_id` 有值 → `inspect_artifact` → 读 `ArtifactSummary.path` 文件内容(容错:读失败/二进制就只用 summary;读取上限如 8KiB 防超大)。理由:cohort(`study: lihc`)在报告正文,不在 summary。
2. 调 `grounding.grounds_hypothesis(&hypothesis.statement, &finding_text)`:
   - `Some(false)`(领域不匹配)→ **不 raise StanceAssessment**;改为 `apply_failures.push(ApplyFailure { hypothesis_id, reason })`,reason 诚实说明"工具输出领域与假设不匹配,已拒绝作为证据"并带可定位线索(如截断的 finding 摘要)。**埋触发点**:reason 用稳定前缀(如 `output-domain-mismatch:`)以便 AS16+ 消费。函数照常返回 Ok(())。
   - `Some(true)` 或 `None` → **维持现状**(raise StanceAssessment),零回归。
3. 该函数需要 `apply_failures: &mut Vec<ApplyFailure>` 入参(目前它只收 `raised_decisions`)——补这个参数,调用方 `auto_run_applied_step` 已持有 `apply_failures`,透传即可。

### 4. CLI：真实 OutputGroundingScorer + 接线

`crates/agentflow-cli`:仿现有 `RelevanceScorer` 的真实实现/wiring(`agent_ops_commands.rs` 的 `relevance_prompt` + 同一 LLM 客户端),新增真实 `OutputGroundingScorer`:
- grounding prompt(中文,问"该发现的领域/队列/疾病是否与假设一致,而非仅主题相关",只答 yes/no);复用同一已配置 LLM 后端(DeepSeek),不引入新配置。
- agent run 全链路(semantic_match 开时)走新的 `run_cycle_with_synth_grounded`,传真实 grounding;未配置 LLM/关闭时传 `NoopOutputGroundingScorer`。

### 5. 不变量

- `argument.rs` 判决确定性:0 LLM/网络(本改动不碰 argument.rs)。
- Noop 默认 → 无 LLM 时行为完全不变。
- 不改证据/事件 payload 结构、不改 DecisionKind、不改 allowlist、不改 AS7-AS14 逻辑。
- 核心无任何具体基因/疾病/study 常量(LLM 自己比对 lihc vs LUAD;finding_text 来自运行产物)。
- 读产物正文仅限 `observation.artifact_id` 指向的本项目产物,容错、限长;不读任意外部路径。

## 测试（离线，stub OutputGroundingScorer）

`agent.rs`:
- **mismatch 拒证**:stub `grounds_hypothesis` 返回 `Some(false)`,跑一个会自动跑出 observation 的 cycle(用 grounded 入口)→ **不产生 StanceAssessment 决策**,且 `apply_failures` 含 `output-domain-mismatch:` 前缀的项。
- **match 维持**:stub 返回 `Some(true)` → 照常 raise StanceAssessment(与现状一致)。
- **Noop 零回归**:用既有 `run_cycle_with_synth`(Noop grounding)→ 行为与改动前完全一致(既有 stance 测试保持绿)。
- artifact 正文读取容错:artifact_id 为 None 或文件不可读时,退回 summary,不 panic。
`agentflow-cli`:既有 cli 测试保持绿;如新增真实 scorer,补最小构造测试。
core 测试数预期 +3~4。

## 验收标准（Claude 复核 + live）

- [ ] fmt / clippy / core / cli / `scripts/acceptance-v1.sh` 全绿；`argument.rs` 仍 0 处 LLM/网络。
- [ ] 单测证明 mismatch→拒证(无 StanceAssessment + apply_failure 前缀)、match→维持、Noop→零回归。
- [ ] **live(编排者,纯 `agent run`,不干预)**:重跑 SPP1/LUAD → 工具仍跑出 lihc 结果,但这次系统**读出 lihc≠LUAD、拒绝当证据、诚实记 apply_failure**,不再静默走 stance 交接。
- [ ] 核心无单次任务常量；Noop 默认无回归；core 测试数不减少。

## 不在本里程碑

- 不做进化本身(复发检测/泛化/扬弃)——AS16-18。
- 不做自纠正的"优先进化近邻/合成全新"分叉——只埋触发点(稳定前缀的 apply_failure)。
- 不新增 DecisionKind(mismatch 暂以 apply_failure 诚实记录;是否升级为独立 handoff 留待 AS16 治理设计)。
- 不改 `examples/tools/*`(工件,工具硬编码问题由进化阶段解决)。
