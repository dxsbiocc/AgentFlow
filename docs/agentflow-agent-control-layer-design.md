# AgentFlow Agent 认知与控制层 — 技术实现规格

Status: Draft for review
Date: 2026-05-30
Scope: Agent cognition & control layer (architecture-spec granularity)

> 本文档**扩展**而非取代 [`agentflow-technical-design.md`](../agentflow-technical-design.md) 的 §15 Agent Layer。
> 它把 §15 已有的零件（Hypothesis Lifecycle、Anti-Self-Deception、Evidence Grade、Dialectical Reasoning）
> 重新组织成一套**四引擎 + 控制宪法**的可落地结构，并补齐设计里缺失的部分：
> `inconclusive` 拆「暂时/本质」、交接引擎的刹车触发策略、轨迹作为安全垫。
>
> 本文档**不重写** runtime / storage / tool registry / 环境后端。这些已在 `agentflow-core` 中实现（M0–M6 / v1 切片）。
> 本层是叠加在现有事件溯源运行时之上的认知与控制层。

---

## 0. 定位与边界

| 维度 | 本层负责 | 本层不碰（沿用现状） |
| --- | --- | --- |
| 数据 | hypothesis / argument / forage-trace / decision-point / checkpoint 投影 | artifacts、runs、tool registry、SQLite 基座 |
| 执行 | 决定「下一步觅食/论证什么」「何时交接用户」 | 实际跑工具、缓存、环境（`runtime/`） |
| 证据 | 把 observation 聚合成对假设的支持/反驳判决 | 产生 observation（`observer/`） |
| 持久化 | 通过 `append_event` 追加新 `event_type`，并建投影 | `ProjectStore` / `EventRecord` 机制本身 |

非目标（V-next 不做）：多 Agent 协同、分布式调度、自然语言对话前端、自动写科研代码。本层先把**单 Agent 的认知-控制闭环**做正确、可审计、可回退。

---

## 1. 设计公理（控制宪法）

所有引擎都服从下面四条，缺一条整套就退化。它们是前期战略讨论的收敛结论，是本层的**不可违反约束（invariants）**。

**A1. 执行权默认归 Agent。** Agent 自主规划、调整、执行用户不想干的活，无需逐步请示。退化后果：少了它，系统变回累人的工具。

**A2. 决策权永远归用户。** 到「需要决策」的岔路，最终由用户拍板，Agent 不替用户做决定。退化后果：少了它，系统变成擅自做主的失控体。

**A3. 建议权常开，按错误代价刹车。** Agent 即使在执行用户既定路线时也保留监督判断：在「错误代价大 / 不可逆 / 会毁目标」处停下给建议（软的，不强制）；在「便宜可逆」处闭嘴放行。触发阈值挂在**错误后果**上，不挂在 Agent 自信度上。退化后果：少了它，系统变成只会闷头干活的应声虫，或变成唠叨的甩手掌柜。

**A4. 全程留可见可回退的轨迹。** Agent 的自主动作必须便宜可见、可撤销，让用户能审计累积漂移并回退。退化后果：少了它，A1 的默认自治会变成静默失控。

> 推论（贯穿全层）：**RAG 被降格为「读地图」这一个 forage action**，不是答案来源。科研价值发生在地图读完、进入未知之后的论证与交接。

---

## 1.5 冲突裁决：本层优先于现状的保守姿态

**优先级规则（权威定调）**：当本层与 `technical-design.md §15` / `framework-review-next-direction` 的保守表述冲突时，**以本层为准**。本层的产品主线是三项**默认自治**的能力，不得被"默认审批"姿态削弱：

1. **自动化科研推进** —— `run_cycle` 默认自主推进，不逐步等批。
2. **动态图** —— Agent 在安全信封内**默认自主**应用 `add_step`/`add_edge`/`update_params`（现有 `graph_patch` 原语已具备），不再逐个 patch 等人审批。
3. **智能分支选择** —— Agent 依据 `Verdict` + 探索/利用策略**自主选择与比较分支**（复用现有 `comparison` 原语），不默认等批。

**§15 / framework-review 在以下点让位（被本层推翻）**：

- 「require approval for new analysis branches」「planner 只能 propose」「Agent Layer should be constrained」作为**默认姿态**——让位给默认自治。
- 「防 fantasy autonomy」作为**首切片的限定**——让位；自动化推进进入前置里程碑（见 §9）。

**以下从现状保留（这些是安全底座/合规/工程，不是"保守 vs 自治"之争，后续讨论本身也认同）**：

- §15 硬禁止：Agent **永不直接操作 DB/shell、永不绕过 registry**——自动化是**驱动** runtime，不是绕过它。
- A2 决策权：高代价/不可逆/动目标的岔路仍交接用户。
- A4 轨迹：全程可见可回退兜底。
- §15 检索合规接口（`ResearchSource` / `AccessStatus` / Literature Retrieval Policy）——见 §5。
- 状态分层（§4.1）与防自欺闸门（§4.5，仅卡高风险声明，不拖慢推进）。

**统一动作表**（§15 审批清单 = 防自欺闸门触发 = 交接 `Raise` 触发，本是同一批动作，合并成一张表）：

| 动作 | 默认自治? | 过防自欺闸门? | 交接用户? |
| --- | --- | --- | --- |
| 已注册工具内、可逆、便宜的执行 | ✅ | ❌ | ❌（A1，轨迹兜底）|
| **动态图变更（add_step/add_edge/update_params）在安全信封内、可回退** | ✅ | ❌ | ❌（A1，轨迹兜底）← 推翻 §15 默认审批 |
| **分支选择 / 比较 / 推进** | ✅ | ❌ | ❌（A1）← 推翻 §15「approval for new branches」|
| 改假设 / 停分支为负证据 | ❌ | ✅ | ✅ |
| 声称 Affirmed / 判 Fundamental | ❌ | ✅ | ✅（达门槛即收口，否则交接）|
| 探索性工具 / 写自定义代码 / 建环境 | ❌ | ✅ | ✅ |
| 破坏性 / 不可逆动作 | ❌ | ✅ | ✅（显式确认）|
| 直接操作 DB/shell/绕过 registry | 🚫 永久禁止 | — | — |

> 一句话：自动驾驶默认开着（动态图 + 智能分支自主推进），刹车（交接）只在真岔路踩，黑匣子（轨迹）全程记录可倒带，方向盘（决策权）最终在用户手里。

---

## 2. 总体架构：四引擎叠加在现有运行时之上

```
┌─────────────────────────────────────────────────────────────┐
│                    控制主循环 (agent::loop)                    │
│   目标 → 觅食 → 论证 → 收敛判定(是/否/无法论证) → 交接? → 回退? │
└───────┬───────────┬───────────┬───────────┬──────────────────┘
        │           │           │           │
   ┌────▼────┐ ┌────▼────┐ ┌────▼────┐ ┌────▼─────┐
   │ 觅食引擎 │ │ 论证引擎 │ │ 交接引擎 │ │轨迹安全垫 │
   │ forage  │ │ argue   │ │ handoff │ │trace_grd │
   └────┬────┘ └────┬────┘ └────┬────┘ └────┬─────┘
        │           │           │           │
        └───────────┴───────────┴───────────┘
                        │
        ┌───────────────▼────────────────────────────┐
        │   现有基座（不改写）                          │
        │   ProjectStore · EventRecord/append_event   │
        │   runtime · observer · graph_patch · tool   │
        └─────────────────────────────────────────────┘
```

新增 `agentflow-core` 模块（与现有 `research.rs` / `graph_patch.rs` 平级）：

| 模块 | 职责 | 对应引擎 |
| --- | --- | --- |
| `hypothesis.rs` | 假设一等实体 + 状态机 | 论证引擎（数据） |
| `argument.rs` | 证据账本、三态判决、防自欺闸门 | 论证引擎（逻辑） |
| `forage.rs` | 检索/验证动作、来源适配器、探索/利用、新鲜度 | 觅食引擎 |
| `handoff.rs` | 决策点、刹车触发策略、决策/体力活分类器 | 交接引擎 |
| `trace_guard.rs` | checkpoint、漂移检测、回退 | 轨迹安全垫 |
| `agent/mod.rs` | 控制主循环，编排上述四者 | 主循环 |

全部沿用现有事件溯源范式：每个动作 `append_event` 一条带新 `event_type` 的 `EventRecord`，读侧用投影函数重建状态。**没有新数据库，没有新进程。**

---

## 3. 共享基座对接：新事件类型与投影

不新增表（V-next）。所有新实体都是 `events` 表上的 `event_type` + `payload_json`，用 `flow_id`/`step_id`/`run_id` 关联现有图。新增 `event_type` 命名空间：

```
hypothesis.created | hypothesis.transitioned
argument.evidence_linked | argument.verdict_rendered
forage.action_started | forage.observation_recorded
handoff.decision_point_raised | handoff.user_resolved
trace.checkpoint_created | trace.reverted
```

投影约定（沿用 `research.rs::list_research_notes` / `observer.rs::list_observations` 模式）：

```rust
// 所有投影都是「重放该类型事件 → 折叠成当前状态」的纯函数。
// 例：impl ProjectStore { pub fn list_hypotheses(&self) -> Result<Vec<Hypothesis>, StorageError> }
```

> 升级路径（非本切片）：若投影重放成本变高，再为热实体加物化表 + 迁移（`storage/migrations.rs` 已具备机制）。先不做，避免过早优化。

---

## 4. 论证引擎 `argument.rs` + `hypothesis.rs`

科研区别于蚂蚁觅食的本质：结局是**三态**（是/否/无法论证），且「否」「无法论证」都是成功终点。论证引擎对**已搜集的证据下判决**，判决决定目标是否达成——而非「找到了多少」。

### 4.1 假设一等实体（对齐并扩展 §15）

```rust
/// 与 technical-design §15 的 7 态对齐，但收敛判定时映射到三态（见 4.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HypothesisStatus {
    Proposed,
    UnderTest,
    Supported,      // → 三态「是」
    Weakened,       // 中间态，趋向「否」
    Contradicted,   // → 三态「否」
    Inconclusive,   // → 三态「无法论证」（需进一步用 InconclusiveKind 细分，见 4.4）
    Superseded,
}

#[derive(Debug, Clone)]
pub struct Hypothesis {
    pub id: String,
    pub statement: String,
    pub origin: String,            // 来自用户目标 / Agent 提出 / 文献
    pub related_goal_id: String,   // 必填：每个假设都挂在一个目标下
    pub status: HypothesisStatus,
    pub confidence: Confidence,    // Low / Medium / High
    pub created_at: i64,
}

impl HypothesisStatus {
    /// 状态机：只允许合法跃迁（参照 domain::StepStatus::can_transition_to 的风格）。
    pub fn can_transition_to(self, next: Self) -> bool { /* ... */ }
}
```

### 4.2 证据账本（Argument Ledger）

每个假设维护两栏证据，每条证据带 §15 的 Evidence Grade，并标明立场。

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceGrade {   // 直接复用 §15 五级
    Observed, Inferred, LiteratureSupported, Hypothesis_, Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stance { Supports, Contradicts, Neutral }

#[derive(Debug, Clone)]
pub struct EvidenceLink {
    pub hypothesis_id: String,
    pub observation_id: Option<String>,  // 关联 observer::ObservationRecord
    pub source: Option<String>,          // 文献/外部来源（forage 产生）
    pub grade: EvidenceGrade,
    pub stance: Stance,
    pub note: String,
}

impl ProjectStore {
    pub fn link_evidence(&self, link: EvidenceLink) -> Result<EventId, StorageError>;
    pub fn evidence_for(&self, hypothesis_id: &str) -> Result<Vec<EvidenceLink>, StorageError>;
}
```

> 「重复研究」检测在此自然落地：一条新证据若只是重复已 `Supported` 假设的同立场证据，可被标注为低增量；它直接服务于用户最初的关切——别重做别人做过的。

### 4.3 三态判决（核心收敛信号）

收敛信号不是「离正结果多近」，而是「**论证是否充分到可下一个可辩护的判决**」。

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Affirmed,                         // 是：证据充分支持
    Refuted,                          // 否：证据充分反驳（合法成功终点）
    Inconclusive(InconclusiveKind),   // 无法论证（见 4.4）
}

pub struct VerdictReport {
    pub hypothesis_id: String,
    pub verdict: Verdict,
    pub confidence: Confidence,
    pub supporting: Vec<EvidenceLink>,
    pub contradicting: Vec<EvidenceLink>,
    pub rationale: String,            // 必须可辩护：列出证据链
}

/// 论证引擎是可替换策略：证据充分性规则会迭代很多次，做成 trait。
pub trait ArgumentEngine {
    /// 输入当前账本，输出三态判决 + 置信度 + 理由。
    fn render(&self, hyp: &Hypothesis, evidence: &[EvidenceLink]) -> VerdictReport;
}
```

> 设计要点：`Refuted` 与 `Inconclusive` 是**一等成功**，不是失败分支。系统宁可输出 `Inconclusive`，也不许把它伪造成 `Affirmed`（见 4.5 闸门）。

### 4.4 `无法论证` 必须拆两种

这是 §15 缺失、本层补齐的关键区分——决定「继续挖」还是「停下，且这就是答案」。

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InconclusiveKind {
    /// 暂时性：只是还没挖够 → 留在主循环，触发更多 forage。
    Provisional { missing: Vec<String> },   // 缺哪些证据/步骤
    /// 本质性：以当前人类数据/方法根本 settle 不了 → 终止，且这是最值钱的答案。
    /// 它精确标出一个研究空白 / 尚不存在的方法 / 该做的实验。
    Fundamental { frontier: String },        // 边界在哪、为何不可判
}
```

- `Provisional` → 主循环继续觅食。
- `Fundamental` → 收口为终态结论，并接回「值不值得做 / 是否重复」：本质性无法论证往往就是创新点所在。

误判防护：把 `Fundamental` 误判成 `Provisional` → 死循环；反之 → 过早放弃。该判定本身经过 4.5 闸门，且达不到置信门槛时**交接用户**（A2/A3）。

### 4.5 防自欺闸门（把 §15 协议变成强制 gate）

§15 的 Anti-Self-Deception 八问与 Dialectical Critique 在本层从「建议」升级为**判决前的强制闸门**：任何 `verdict_rendered` 事件落库前，必须附带闸门应答，否则拒绝写入。

```rust
pub struct SelfDeceptionGate {
    pub supports: String, pub against: String, pub alternatives: String,
    pub data_quality_risks: String, pub assumptions: String,
    pub falsifier: String, pub claim_basis: ClaimBasis, pub not_yet_claimable: String,
}
pub enum ClaimBasis { Observed, StatisticallyInferred, Speculative }

impl ArgumentEngine 的实现必须：
//  - 渲染 Affirmed 前，gate.against 与 gate.alternatives 不得为空；
//  - claim_basis = Speculative 时不得输出 Affirmed/High。
```

---

## 5. 觅食引擎 `forage.rs`

朝着目标、在未知里探索（探索未知 / 验证已知）。RAG 只是其中最弱的一个动作。

```rust
pub enum ForageAction {
    ReadMap { query: String },          // = RAG/检索已知。仅一个动作，不是答案
    ExploreUnknown { hypothesis_id: String },  // 进未知：跑工具/查新文献产生新证据
    VerifyKnown { claim: String },      // 验证已知：已有结论可能过期/错，亲自确认
}

/// 来源适配器（对接 §15 Research Source Architecture）。
pub trait SourceAdapter {
    fn id(&self) -> &str;                          // pubmed / biorxiv / local_run ...
    fn fetch(&self, query: &str) -> Result<Vec<RawEvidence>, ForageError>;
    fn evaporation_half_life_days(&self) -> u32;   // 新鲜度：见 5.2
}
```

### 5.1 探索 / 利用（防重复研究）

觅食策略保留 ε 探索项：大部分跟随强信号（利用），小概率走进信号稀薄区（探索）。ε=0 的觅食 = 只在最热门方向重复别人。

```rust
pub struct ForagePolicy { pub epsilon: f32 /* 强制 > 0 */ }
```

### 5.2 新鲜度 = 挥发（解决「滞后」与「重复」同一机制）

向量库/已读证据不是静态知识，是会**挥发的信息素地图**，挥发率按领域配（AI 快、数学慢）。挥发到阈值以下的证据需重新觅食。

```rust
/// strength(t) = strength0 * 0.5^( age_days / half_life_days )
pub fn current_strength(strength0: f64, age_days: f64, half_life_days: u32) -> f64;
```

「滞后」= 读了挥发干净的信息素；「重复」= 在信息素已爆满处再走一遍。两者同一参数处理。

---

## 6. 交接引擎 `handoff.rs`

实现宪法 A1–A3：执行权默认归 Agent，决策权归用户，建议权常开按错误代价刹车。**核心难点不是「该不该问」（答案：该），而是「何时问」。**

### 6.1 决策点与控制判决

```rust
pub struct DecisionPoint {
    pub id: String,
    pub kind: DecisionKind,
    pub digest: String,              // 已干过的活（凭证，A3：不许没尝试就上交）
    pub options: Vec<HandoffOption>, // 嚼碎的选项，非裸问题（区分顾问 vs 甩手掌柜）
    pub recommendation: usize,       // Agent 必须有观点，仍带推荐
}

pub struct HandoffOption {
    pub label: String, pub direction: String,
    pub cost: Cost, pub risk: Risk, pub reversible: bool,
}

pub enum DecisionKind {
    DeepenOrStop,        // 无法论证(暂时)，下一步贵/分叉多
    PremiseChallenged,   // 证据显示用户前提可能不成立 ← 最值钱，替用户挡跑偏
    BudgetThreshold,     // 撞到时间/成本阈值
    GoalMutation,        // 动到核心目标/假设本身
}
```

### 6.2 刹车触发策略（A3 的可执行化）

触发挂在**错误后果**而非 Agent 自信度上：

```rust
pub trait InterventionPolicy {
    /// 返回 Proceed（自治往前）或 Raise(DecisionPoint)（交接用户）。
    fn evaluate(&self, ctx: &StepContext) -> ControlVerdict;
}

pub enum ControlVerdict { Proceed, Raise(DecisionPoint) }
```

判定规则（默认实现）：

| 条件 | 动作 |
| --- | --- |
| 下一步明确且**便宜且可逆** | `Proceed`（不问，自己走）|
| 昂贵 / 不可逆 / 多条等价分叉 | `Raise(DeepenOrStop)` |
| 证据与用户前提冲突 | `Raise(PremiseChallenged)` |
| 动到核心目标/假设 | `Raise(GoalMutation)` |
| 即将越过预算阈值 | `Raise(BudgetThreshold)` |

> 用户既定路线：默认按用户方案走（A1/A2）；仅当沿途出现高代价/不可逆错误时 `Raise`，且为软建议——用户可无视。

### 6.3 体力活 vs 决策 分类器

避免「越权（把决策当体力活）」与「甩手掌柜（把体力活当决策）」：

```rust
/// 判据：此处不同的合理选择，是否实质改变用户在乎的结果？
///  - 会 → Decision（浮出/交接）
///  - 不会 / 用户明显不在乎 → Labor（自己干掉）
pub fn classify(task: &PendingTask, goal: &Goal) -> TaskClass;
pub enum TaskClass { Labor, Decision }
```

---

## 7. 轨迹安全垫 `trace_guard.rs`

轨迹的**唯一站得住的作用**：不是当产品卖，而是让 A1 的默认自治变安全——提供「累积漂移可见」与「可回退」。

```rust
pub struct Checkpoint { pub id: String, pub at_event: EventId, pub label: String }

pub struct DriftReport {
    pub from: Checkpoint,
    pub net_goal_delta: String,   // 自该 checkpoint 起，相对用户目标净漂移了多少
    pub sub_threshold_steps: u32, // 多少步单独看都低于刹车阈值
    pub should_surface: bool,     // 累积漂移是否已大到该主动交接
}

impl ProjectStore {
    pub fn create_checkpoint(&self, label: &str) -> Result<Checkpoint, StorageError>;
    pub fn detect_drift(&self, from: &Checkpoint) -> Result<DriftReport, StorageError>;
    pub fn revert_to(&self, checkpoint_id: &str) -> Result<(), StorageError>; // 事件溯源天然可回放
}
```

**静默漂移守护**：许多次单独低于刹车阈值的自主微调，累加可能已偏离用户本意。`detect_drift` 把累积量重新拉到刹车判定里，`should_surface` 为真时主动 `Raise` 一个交接点。回退依赖事件溯源——`revert_to` 即重放到某 `EventId`。

---

## 8. 控制主循环 `agent/mod.rs`

把四引擎编排成以「目标达成（三态判决达置信门槛）」为收敛条件的循环。

```rust
pub enum LoopOutcome {
    Reached(Verdict),          // 是/否/本质性无法论证，达门槛 → 收口出报告
    HandedOff(DecisionPoint),  // 交接用户，挂起等决策
}

pub fn run_cycle(store: &ProjectStore, goal: &Goal) -> Result<LoopOutcome, AgentError> {
    // 1. 分解目标 → 假设/子目标（hypothesis.created）
    // 2. forage：ReadMap → ExploreUnknown/VerifyKnown（按 ForagePolicy，含 ε 探索、挥发过滤）
    // 3. link_evidence：observation/来源 → 证据账本
    // 4. argue：ArgumentEngine.render → Verdict（过防自欺闸门）
    // 5. 收敛判定：
    //      Affirmed/Refuted/Inconclusive(Fundamental) 且达门槛 → Reached
    //      Inconclusive(Provisional)                          → 回到步骤 2
    // 6. 每步 InterventionPolicy.evaluate：Raise → HandedOff（带凭证+推荐）
    // 7. trace_guard.detect_drift：should_surface → 主动 HandedOff
}
```

终止条件（与宪法一致）：
- 达成 = 三态任一达置信门槛且过闸门（**伪造 Affirmed = 失败**）。
- 该停不停（`Provisional` 死循环）= 失败 → 由预算阈值/漂移检测强制交接。
- 需要决策 = 一律 `HandedOff`，遵循用户（A2）。

> **自动化推进（§1.5 主线在循环里的落点）**：步骤 2–5 每一轮，Agent 依据 `Verdict` 与探索/利用策略**自主选择下一个分支**并**自主变更动态图**（`add_step`/`add_edge`/`update_params`，复用现有 `graph_patch` / `comparison` 原语），在安全信封内**默认执行、不逐步等批**；仅当分支决策触及 `GoalMutation` / 不可逆 / 高代价时才 `Raise` 交接。这就是"自动化科研推进 + 动态图 + 智能分支选择"在主循环中的具体形态。

---

## 9. 与现有里程碑的衔接（轻量排序）

接在现状（M6 报告核心 / v1 可用切片）之后。**自动化推进是主线，不后置**（§1.5）；安全由「交接 + 轨迹」保证，而非由「默认审批」保证：

1. **H1 假设与证据账本** ✅ **已实现并验收（2026-05-31，见 `status/h1-hypothesis-argument-plan.md`）**：`hypothesis.rs`（7 态生命周期 + 状态机）+ `argument.rs`（证据账本 + `RuleBasedEngine` 规则版三态判决）。core 测试 95→109，质量门全绿。——这是分支选择的**依据**，故排第一。（证据与现有 `observation` 的打通在 H6 觅食阶段完善。）
2. **H2 智能分支选择 + 动态图提议** ✅ **已实现并验收（2026-05-31，见 `status/h2-branch-selection-plan.md`）**：`branch.rs` —— Agent 用 `Verdict` 自主选「深入/派生/放弃/保持」+ 确定性评分排序 + 探索提升 + `propose_branch_patch`（复用现有 `graph_patch`）。core 测试 109→116。
   - **工程裁决**：H2 **只提议、不自动应用**（`apply_graph_patch` 留到 H7），Abandon 只出建议不自动改状态（A2）。**自主 apply 默认开启**需 H3（刹车）+ H4（轨迹回退）就位才安全（A4：默认自治须可见可回退）——这是诚实的安全排序，非退回保守。
3. **H3 交接引擎** ✅ **已实现并验收（2026-05-31，见 `status/h3-handoff-engine-plan.md`）**：`handoff.rs` —— `DefaultPolicy` 刹车策略（优先级 goal→premise→budget→贵/不可逆/分叉）+ 决策点提出/解决（事件溯源）+ 决策/体力活分类器；强制 A3（digest 非空、必带推荐、选项非空）。core 测试 116→129。CLI 暴露决策点与 resolve **延后到统一 CLI 里程碑**（H1–H3 均暂未做 CLI）。
4. **H4 轨迹安全垫**：`trace_guard.rs` checkpoint / drift / revert——让 H2 的默认自治**可回退**。
5. **H5 防自欺闸门**：`verdict_rendered` 落库强制带 `SelfDeceptionGate`，仅卡高风险声明（改假设/Affirmed/Fundamental），不拖慢推进；报告区分 Evidence Grade。
6. **H6 觅食引擎**：`forage.rs` 采纳 §15 `ResearchSource` 接口 + `AccessStatus` + 挥发/ε 探索（先 1 个来源，如 PubMed `ReadMap`），喂证据账本。
7. **H7 控制主循环**：`agent/mod.rs` 串起来，单 Agent 闭环跑通一个真实科研目标。

> 排序逻辑：先建分支选择**所依赖**的证据/判决层（H1），随即放开自动化推进（H2）；安全不靠"默认审批"，而靠交接（H3）+ 轨迹（H4）。这与现状 `framework-review` 的"先看见再自治、防幻想自治"姿态相反——按 §1.5，本层为准。

---

## 10. 反模式与守护（验收时逐条核对）

| 反模式 | 由哪条公理/机制守护 |
| --- | --- |
| 甩手掌柜（裸问题上交） | A3 + `DecisionPoint.digest` 必填 + `recommendation` 必带 |
| 应声虫（只闷头干活不出声） | A3 + `InterventionPolicy` 在高代价处强制 `Raise` |
| 越权（替用户做决定） | A2 + `classify` 把决策浮出 + 主循环需决策一律 `HandedOff` |
| 伪造「是」（幻觉） | 4.5 闸门 + `Refuted`/`Inconclusive` 为一等成功终点 |
| 死循环（该停不停） | 4.4 `Fundamental` 终态 + 预算阈值 + 漂移检测 |
| 静默漂移 | A4 + `trace_guard.detect_drift.should_surface` |
| RAG 当答案 | §1 推论：`ReadMap` 仅为一个 forage action |
| 重复研究 | 证据账本低增量标注 + ε 探索 + 挥发新鲜度 |

---

## 11. 未决问题（需用户拍板）

1. **用户离线、又必须决策才能继续时**，交接引擎应：(a) 停下挂起任务等用户回来（严守 A2，绝不替决策）；还是 (b) 允许在「明确可逆」前提下先替用户走一步、标成「待追认」？——这是 A2 唯一会被现实拉扯的地方。倾向 (a) 为默认，(b) 作为用户可显式开启的选项。
2. **论证引擎的「证据充分性」**：先做可形式化规则版（统计显著 / 可重复 / 无矛盾证据计数），还是一开始就允许软判断？建议规则版起步，trait 化以便替换。
3. **置信门槛**与**挥发半衰期**是否暴露为用户可配项（按领域），还是先用内置默认。

---

## 附：术语对照（本层 ↔ §15 ↔ 战略讨论）

| 本层 | technical-design §15 | 战略讨论比喻 |
| --- | --- | --- |
| `Verdict::{Affirmed,Refuted,Inconclusive}` | supported / contradicted / inconclusive | 论证为是/否/无法论证 |
| `InconclusiveKind::Fundamental` | （无） | 本质性无法论证 = 研究空白/创新点 |
| `argument.rs` | Anti-Self-Deception + Dialectical | 论证引擎（蚂蚁没有的那台） |
| `forage.rs` | Research Source Architecture | 觅食引擎；RAG = 读地图 |
| `handoff.rs` | Approval Policy | 交接引擎；顾问而非甩手掌柜 |
| `trace_guard.rs` | （事件溯源散见各处） | 轨迹安全垫 |
