# H1 实现简报：假设与证据账本 + 规则版三态判决

Status: Implemented + verified (2026-05-31)
Date: 2026-05-30
Owner(orchestrator): Claude · Executor: Codex

## 验收记录（Claude 独立复验 2026-05-31）

Codex 交付，编排者独立复跑 + 代码审核，**逐条通过**：

- ✅ `cargo clippy --workspace --all-targets -- -D warnings`：无警告（独立复跑）。
- ✅ `cargo test --workspace`：core 109（基线 95，+14 新测试）/ cli 22 / schemas 3 全绿。
- ✅ 仅改 3 文件：`hypothesis.rs`、`argument.rs`、`lib.rs`（注册模块）。
- ✅ 无新增依赖（4 个 Cargo.toml 零变更）、无 serde、无新建表。
- ✅ 事件类型精确为 4 个具名常量：`hypothesis.created` / `hypothesis.transitioned` / `argument.evidence_linked` / `argument.verdict_rendered`。
- ✅ `can_transition_to` 状态机与规范逐条一致；非法跃迁被拒（有测试）。
- ✅ `RuleBasedEngine` 五条判定规则（空/Affirmed/Refuted/零权重 Inconclusive/below-margin）与规范权重、margin、置信度逐字一致，各有测试。
- ✅ **C2 不变量成立**：`render_verdict` 只追加判决事件，不修改 hypothesis 生命周期状态。
- ✅ `link_evidence` 先校验请求 + `inspect_hypothesis` 存在性再写入。

结论：合并就绪。`Fundamental` 按 H1 仅保留类型与序列化往返（规则引擎不产出），符合排除项。
Spec source: [`agentflow-agent-control-layer-design.md`](../agentflow-agent-control-layer-design.md) §4 / §9-H1

## 目标

实现 Agent 控制层的**论证引擎数据面 + 规则版三态判决**，作为后续「智能分支选择」（H2）的依据层。**纯 `agentflow-core` 库代码 + 单元测试**；本里程碑**不做 CLI、不做防自欺强制闸门（H5）、不做觅食（H6）**。

## 硬约束（违反即返工）

1. **不新增任何 crate 依赖**。`agentflow-core` 现仅依赖 `rusqlite` + `agentflow-schemas`。禁止引入 `serde`/`serde_json` 等。
2. **JSON 手写**，沿用现有约定：参照 `research.rs` 末尾的私有助手 `escape_json` / `json_string_field` / `json_nullable_string_field` / `unescape_json_string` / `optional_json_string`，在新模块内复制所需的最小私有助手（项目当前就是各模块各自私有复制，保持一致，**不要**为此重构出共享模块）。
3. **事件溯源**：所有写操作通过 `ProjectStore::append_event(EventRecord{flow_id,step_id,run_id,event_type,payload_json})`，读操作通过对 `events` 表按 `event_type` 投影重建（参照 `research.rs::list_research_notes` / `inspect_research_note`）。**不新增数据库表**。
4. **错误处理**：用 `StorageError`（`InvalidInput` / `NotFound` 等），校验风格参照 `research.rs::validate_research_note_request` / `validate_non_empty`。
5. **测试**：每个新模块内 `#[cfg(test)] mod tests`，参照 `research.rs` 测试风格（用临时目录 `ProjectStore::init`）。新代码测试覆盖 ≥ 80%，覆盖正常路径 + 校验失败 + 非法状态跃迁 + 各判决分支。
6. **质量门必须全绿**：`cargo test --workspace` 与 `cargo clippy --workspace --all-targets -- -D warnings`。当前基线：core 95 / cli 22 / schemas 3 测试通过，clippy 无警告。**不得降低或破坏现有测试**。
7. `unsafe_code = forbid`（workspace 已设）。时间戳用现有 `now_unix_seconds()`，id 生成参照现有 `event_{nanos}` 风格（事件 id 由 `append_event` 生成；实体 id 取所属 `created` 事件的 id）。

## 交付物

### 1. `crates/agentflow-core/src/hypothesis.rs`（新建）

```rust
pub enum Confidence { Low, Medium, High }          // as_str / parse，风格仿 domain.rs 的枚举
pub enum HypothesisStatus {                         // 7 态，对齐 technical-design §15
    Proposed, UnderTest, Supported, Weakened, Contradicted, Inconclusive, Superseded,
}
// HypothesisStatus: as_str / parse / can_transition_to(self, next) -> bool

pub struct Hypothesis {
    pub id: String,             // = hypothesis.created 事件 id
    pub statement: String,
    pub origin: String,         // 来源描述：user_goal / agent / literature 等自由文本
    pub related_goal_id: String,// 必填非空：每个假设挂在一个目标下
    pub status: HypothesisStatus,
    pub confidence: Confidence,
    pub created_at: i64,
    pub updated_at: i64,        // 最近一次事件时间
}

pub struct HypothesisRequest {  // status 默认 Proposed，confidence 默认 Low
    pub statement: String, pub origin: String, pub related_goal_id: String,
}
```

`impl ProjectStore`：
- `record_hypothesis(&self, req: HypothesisRequest) -> Result<Hypothesis, StorageError>`
  - 校验 statement/origin/related_goal_id 非空；append `event_type = "hypothesis.created"`，payload 存上述字段 + 初始 status/confidence。
- `list_hypotheses(&self) -> Result<Vec<Hypothesis>, StorageError>`
  - 投影：折叠 `hypothesis.created`（基态）+ `hypothesis.transitioned`（按 payload 内 `hypothesis_id` 更新 status/confidence/updated_at），按 created_at 升序。
- `inspect_hypothesis(&self, id: &str) -> Result<Hypothesis, StorageError>`（NotFound 处理仿 research）。
- `transition_hypothesis(&self, id: &str, next: HypothesisStatus, confidence: Confidence) -> Result<Hypothesis, StorageError>`
  - 先 `inspect_hypothesis` 取当前态；若 `!current.status.can_transition_to(next)` → `InvalidInput`；append `event_type = "hypothesis.transitioned"`，payload 含 `hypothesis_id`/`status`/`confidence`；返回投影后的最新实体。

`can_transition_to` 规则（终态 Superseded 不可再转出；任意非终态可转 Superseded）：
- Proposed → UnderTest | Superseded
- UnderTest → Supported | Weakened | Contradicted | Inconclusive | Superseded
- Weakened → UnderTest | Contradicted | Inconclusive | Superseded
- Supported → Weakened | Superseded
- Contradicted → Superseded
- Inconclusive → UnderTest | Superseded
- Superseded → （无）
- 同态转同态返回 false。

### 2. `crates/agentflow-core/src/argument.rs`（新建）

```rust
pub enum EvidenceGrade { Observed, Inferred, LiteratureSupported, Hypothesis, Unsupported }
// 权重：Observed=3, Inferred=2, LiteratureSupported=1, Hypothesis=0, Unsupported=0
pub enum Stance { Supports, Contradicts, Neutral }

pub struct EvidenceLinkRequest {
    pub hypothesis_id: String,
    pub observation_id: Option<String>,  // 关联 observer::ObservationRecord（H1 只存字符串，不强校验存在）
    pub source: Option<String>,          // 文献/外部来源
    pub grade: EvidenceGrade,
    pub stance: Stance,
    pub note: String,
}
pub struct EvidenceLink {
    pub id: String, pub hypothesis_id: String,
    pub observation_id: Option<String>, pub source: Option<String>,
    pub grade: EvidenceGrade, pub stance: Stance, pub note: String, pub created_at: i64,
}

pub enum InconclusiveKind {
    Provisional { missing: Vec<String> },  // 还没挖够，继续觅食
    Fundamental { frontier: String },      // 本质不可判（H1 规则引擎不产出，仅类型预留）
}
pub enum Verdict { Affirmed, Refuted, Inconclusive(InconclusiveKind) }

pub struct VerdictReport {
    pub hypothesis_id: String, pub verdict: Verdict, pub confidence: Confidence,
    pub supporting: Vec<EvidenceLink>, pub contradicting: Vec<EvidenceLink>,
    pub rationale: String,   // 可辩护：列出分数/计数/命中规则
}

pub trait ArgumentEngine {
    fn render(&self, hypothesis_id: &str, evidence: &[EvidenceLink]) -> VerdictReport;
}
pub struct RuleBasedEngine;  // 默认确定性实现；阈值用模块常量
```

`RuleBasedEngine::render` 确定性规则：
- `support = Σ weight(grade)`（stance=Supports）；`contra = Σ weight(grade)`（stance=Contradicts）；Neutral 不计分。
- `has_obs_support` / `has_obs_contra` = 是否存在对应立场且 grade=Observed 的证据。
- 常量：`AFFIRM_MARGIN = 3`，`REFUTE_MARGIN = 3`，`STRONG_MARGIN = 6`。
- 判定（按序）：
  1. 证据为空 → `Inconclusive(Provisional{missing: ["no evidence linked yet"]})`，Low。
  2. `support - contra >= AFFIRM_MARGIN && has_obs_support` → `Affirmed`；置信度 = (margin>=STRONG_MARGIN && contra==0)?High:Medium。
  3. `contra - support >= REFUTE_MARGIN && has_obs_contra` → `Refuted`；置信度同上（看 support==0）。
  4. `support==0 && contra==0`（只有 0 权重 grade）→ `Inconclusive(Provisional{missing:["only weak/unsupported grades; need observed/inferred evidence"]})`，Low。
  5. 其它（有分但未达 margin 或缺 observed）→ `Inconclusive(Provisional{missing:["evidence below decision margin; need stronger or more decisive evidence"]})`，Medium。
- `Fundamental` 在 H1 **不由规则引擎产出**（需 H6 觅食/人工显式信号），仅保留类型与 `Inconclusive` 的序列化往返。
- `rationale`：确定性字符串，含 support/contra 分数、证据条数、命中的规则编号。
- `render` 内部把 evidence 拆进 `supporting`/`contradicting` 返回。

`impl ProjectStore`：
- `link_evidence(&self, req: EvidenceLinkRequest) -> Result<EvidenceLink, StorageError>`
  - 校验 `hypothesis_id` 存在（调用 `inspect_hypothesis`，不存在 → NotFound 透传）；note 非空；append `event_type = "argument.evidence_linked"`。
- `evidence_for(&self, hypothesis_id: &str) -> Result<Vec<EvidenceLink>, StorageError>`（投影，按 created_at 升序）。
- `render_verdict(&self, hypothesis_id: &str, engine: &dyn ArgumentEngine) -> Result<VerdictReport, StorageError>`
  - 取 `evidence_for`，调 `engine.render`，append `event_type = "argument.verdict_rendered"`（payload 存 verdict 文本 + confidence + rationale + hypothesis_id），返回报告。
  - 注意：本里程碑**不**强制要求 SelfDeceptionGate（那是 H5）；但 `render_verdict` 不得修改 hypothesis 的 status（生命周期与判决分层，见 §4.1/C2）。

### 3. `crates/agentflow-core/src/lib.rs`

新增 `pub mod hypothesis;` 与 `pub mod argument;`（与现有 `pub mod research;` 同位置）。

## 验收标准（Claude 审核时逐条核对）

- [ ] `cargo clippy --workspace --all-targets -- -D warnings` 无警告。
- [ ] `cargo test --workspace` 全绿，且 core 测试数较基线（95）净增（新增模块测试）。
- [ ] 无新增 crate 依赖（`git diff` 检查两个 Cargo.toml 无变化）。
- [ ] 事件类型严格为 `hypothesis.created` / `hypothesis.transitioned` / `argument.evidence_linked` / `argument.verdict_rendered`。
- [ ] 非法状态跃迁被拒（有对应测试）。
- [ ] 三态判决五条规则各有测试覆盖（Affirmed/Refuted/两类 Inconclusive/below-margin）。
- [ ] `render_verdict` 不改 hypothesis 生命周期状态。
- [ ] 代码风格与 `research.rs` 一致（私有 JSON 助手、校验函数、投影查询）。

## 不在本里程碑（明确排除）

CLI 命令、SelfDeceptionGate 强制闸门、forage/检索、动态图分支选择（H2）、置信门槛可配、物化表。
