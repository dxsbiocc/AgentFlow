# H6 实现简报：觅食引擎（契约 + 合规 + 新鲜度 + 证据桥）

Status: Implemented + verified (2026-05-31)
Date: 2026-05-31
Owner(orchestrator): Claude · Executor: Codex
Spec source: [`agentflow-agent-control-layer-design.md`](../agentflow-agent-control-layer-design.md) §5 / §9-H6
Depends on: H1（argument.rs 证据账本，已验收）

## 验收记录（Claude 独立复验 2026-05-31）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **151**（基线 141，+10）/ cli 22 / schemas 3 全绿。
- ✅ Cargo 零变更；core 无 HTTP；无新表；event_type 精确 `forage.action_started` / `forage.observation_recorded`。
- ✅ `grade_from_access` 七态：全文→LiteratureSupported，abstract→Hypothesis，metadata/unavailable/failed→Unsupported。
- ✅ **§15 合规回归**：`abstract_forage_evidence_cannot_affirm_verdict` + `literature_supported_forage_evidence_alone_cannot_affirm_verdict` 两测试断言纯文献证据无法单独判 Affirmed。
- ✅ `current_strength` 半衰期数学（半衰/half_life=0 不挥发/age=0 原值）。
- ✅ `link_forage_evidence` 复用 `link_evidence`，grade 由 access_status 推导。
- ✅ forage.rs 无 `apply_graph_patch` / `transition_hypothesis`。

结论：合并就绪。实际 PubMed/bioRxiv 检索工具脚本 + observer 适配器为后续集成步（已登记排除项）。

## 架构路线（已定，不可违反）

**检索作为注册工具（外置进程），复用现有 flow/step/observer 管线，core 零新增依赖。** 因此本里程碑 **core 不做任何 HTTP**，只实现觅食的**契约 + §15 合规 + 新鲜度 + 证据桥**。实际 PubMed/bioRxiv 检索工具（外部脚本）+ 其 observer 适配器是**后续集成步**，不在 H6。

## 目标

`forage.rs`：定义觅食动作与来源访问状态（§15）、把访问状态映射成合规的证据 grade（abstract 不能撑强结论）、新鲜度/挥发计算、ε 探索策略、觅食事件记录与投影、以及「觅食观察 → 证据账本」的桥。纯 `agentflow-core` 库 + 测试，无 CLI。

## 硬约束（与既往一致）

1. 不新增任何 crate 依赖（禁 HTTP 客户端/serde/rand）；JSON 手写复用同款私有助手。
2. 事件溯源；新增 event_type **仅** `forage.action_started` / `forage.observation_recorded`；不新增表。
3. `StorageError` 校验风格仿 `research.rs`。
4. `#[cfg(test)] mod tests` 覆盖 ≥80%。
5. 质量门全绿：`clippy -D warnings` + `cargo test`。**基线 core 141 / cli 22 / schemas 3，不得破坏。**
6. `unsafe_code = forbid`；时间戳 `now_unix_seconds()`。
7. 禁止调用 `apply_graph_patch` / `transition_hypothesis`。

## 交付物（`crates/agentflow-core/src/forage.rs` 新建）

```rust
pub enum AccessStatus {                 // §15 七态；as_str/parse
    MetadataOnly, AbstractAvailable, OpenAccessFullText,
    UserProvidedFullText, SubscriptionConnectorFullText,
    FullTextUnavailable, RetrievalFailed,
}

pub enum ForageAction {                 // as_str/parse
    ReadMap,          // = 检索已知（RAG），仅一个动作
    ExploreUnknown,   // 进未知
    VerifyKnown,      // 验证已知
}

pub struct ForageObservation {
    pub id: String,                     // = forage.observation_recorded 事件 id
    pub source_id: String,              // pubmed / biorxiv / local ...
    pub external_id: String,            // DOI / PMID / URL
    pub title: String,
    pub access_status: AccessStatus,
    pub retrieved_at: i64,
}
```

合规映射（§15 核心：abstract 仅用于 triage，不能撑强结论）：

```rust
/// 访问状态 → 证据 grade。复用 argument::EvidenceGrade。
pub fn grade_from_access(status: AccessStatus) -> EvidenceGrade;
//  OpenAccessFullText / UserProvidedFullText / SubscriptionConnectorFullText
//        -> EvidenceGrade::LiteratureSupported   （权重 1：外部文献支持，非项目内观测）
//  AbstractAvailable     -> EvidenceGrade::Hypothesis    （权重 0：仅 triage，撑不起 Affirmed）
//  MetadataOnly          -> EvidenceGrade::Unsupported   （权重 0）
//  FullTextUnavailable / RetrievalFailed -> EvidenceGrade::Unsupported
```

> 这条映射保证：纯文献证据（即便全文）权重最高只到 LiteratureSupported(1)，单独达不到规则引擎 `AFFIRM_MARGIN=3` 且缺 Observed → 无法仅凭文献判 Affirmed，自动落实 §15「实现复杂方法前需全文或独立验证」。

新鲜度/挥发：

```rust
/// strength(t) = strength0 * 0.5^(age_days / half_life_days)
pub fn current_strength(strength0: f64, age_days: f64, half_life_days: u32) -> f64;
// half_life_days == 0 视为不挥发（返回 strength0）；age<=0 返回 strength0。
```

ε 探索（确定性，与 branch 一致，不引 RNG）：
```rust
pub struct ForagePolicy { pub explore_enabled: bool }
```

`impl ProjectStore`：
- `record_forage_action(&self, action: ForageAction, query: &str, source_id: &str) -> Result<String, StorageError>`
  - 校验 query/source_id 非空；append `forage.action_started`（payload 含 action/query/source_id）；返回事件 id。
- `record_forage_observation(&self, source_id: &str, external_id: &str, title: &str, access_status: AccessStatus) -> Result<ForageObservation, StorageError>`
  - 校验非空；append `forage.observation_recorded`；返回。
- `list_forage_observations(&self) -> Result<Vec<ForageObservation>, StorageError>`（投影，created_at 升序）。
- `inspect_forage_observation(&self, id: &str) -> Result<ForageObservation, StorageError>`（NotFound 仿 research）。
- `link_forage_evidence(&self, hypothesis_id: &str, forage_observation_id: &str, stance: Stance, note: &str) -> Result<EvidenceLink, StorageError>`
  - 取觅食观察（NotFound 透传）；`grade = grade_from_access(obs.access_status)`；调用现有 `link_evidence`（`observation_id = forage_observation_id`，`source = Some(external_id)`，grade，stance，note）。这是觅食 → 证据账本的桥。

## 验收标准（Claude 审核逐条核对）

- [ ] clippy `-D warnings` 无警告；`cargo test` core 较 141 净增，全绿。
- [ ] 无新依赖、无新表；新 event_type 仅 `forage.action_started` / `forage.observation_recorded`。
- [ ] `grade_from_access` 七态映射逐条有测试；尤其断言「全文 → LiteratureSupported」「abstract → Hypothesis」。
- [ ] `current_strength` 有测试：half_life 处恰好半衰、half_life=0 不挥发、age=0 原值。
- [ ] `link_forage_evidence` 端到端：记录觅食观察 → 桥成 EvidenceLink，grade 来自 access_status；随后 `render_verdict` 仅凭 abstract 证据**不能**判 Affirmed（合规回归测试）。
- [ ] grep 确认 forage.rs 无 HTTP/依赖、无 `apply_graph_patch`/`transition_hypothesis`。

## 不在本里程碑（明确排除）

实际 PubMed/bioRxiv 检索工具脚本 + 其 observer 适配器（后续集成步）、CLI、与主循环集成、真随机 ε、用户 PDF 导入为全文证据（后续）。
