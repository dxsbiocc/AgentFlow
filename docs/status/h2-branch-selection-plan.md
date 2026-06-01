# H2 实现简报：智能分支选择 + 动态图提议

Status: Implemented + verified (2026-05-31)
Date: 2026-05-31
Owner(orchestrator): Claude · Executor: Codex
Spec source: [`agentflow-agent-control-layer-design.md`](../agentflow-agent-control-layer-design.md) §9-H2
Depends on: H1（hypothesis.rs / argument.rs，已实现验收）

## 验收记录（Claude 独立复验 2026-05-31）

- ✅ `clippy -D warnings` 无警告（独立复跑）；`cargo test` core **116**（基线 109，+7）/ cli 22 / schemas 3 全绿。
- ✅ 仅改 `argument.rs` / `branch.rs` / `lib.rs`；4 个 Cargo.toml 零变更；无新表、无新 event_type。
- ✅ **安全裁决①**：`branch.rs` 不含 `apply_graph_patch`（只提议不应用）。
- ✅ **安全裁决②**：`branch.rs` 不含 `transition_hypothesis`；`propose_branch_patch` 对 Abandon/Hold 返回 `InvalidInput`。
- ✅ verdict→kind 五映射逐条一致；无判决 → Hold。
- ✅ `latest_verdict_for` 按 `created_at DESC` + `hypothesis_id` 筛选，取该假设最新判决。
- ✅ 评分/排序确定性；`propose_branch_patch` 序列化为合法 `add_step` 提议走 `propose_graph_patch`。

已知特性（非问题）：`branch_candidates` 逐假设扫描判决事件，整体 O(n²)；本地规模无碍，符合「投影成本变高再物化」预案。

结论：合并就绪。

## 目标

实现**判决驱动的智能分支选择** + **动态图变更的提议管线**。Agent 依据 H1 的假设与三态判决，决定下一步对哪个假设「深入 / 派生 / 放弃 / 保持」，并把「深入/派生」翻译成对现有 `propose_graph_patch` 的**提议**。

## 编排者的工程裁决（不可违反，写在最前）

- **H2 只提议，不自动应用**：图变更一律走现有 `propose_graph_patch`（审批门控），**禁止**调用 `apply_graph_patch` 自动落图。自动应用默认开启要等 H3（刹车）+ H4（轨迹回退）就位（A4：默认自治须可见可回退）。
- **Abandon 不自动转生命周期**：放弃只产出**建议**（recommend_status），**禁止**在 H2 里调用 `transition_hypothesis` 自动改状态——那是需用户决策的动作（A2）。
- **不做真随机 ε**：探索用**确定性**实现（见下），不引入 RNG 依赖，保证测试可复现。

## 硬约束（与 H1 相同，违反即返工）

1. 不新增任何 crate 依赖（禁止 serde/rand 等）。JSON 手写，复用同款私有助手写法。
2. 事件溯源：读走 events 投影；写**仅**通过现有 `propose_graph_patch`（不新增 event_type）。不新增数据库表。
3. 错误用 `StorageError`；校验风格仿 `research.rs`。
4. 每个新模块内 `#[cfg(test)] mod tests`，覆盖 ≥ 80%：每种 verdict→kind 映射、评分排序、explore 提升、propose 成功、Abandon/Hold 拒绝出 patch、无 verdict→Hold。
5. 质量门全绿：`cargo clippy --workspace --all-targets -- -D warnings` 与 `cargo test --workspace`。**基线：core 109 / cli 22 / schemas 3，clippy 无警告。不得破坏现有测试。**
6. `unsafe_code = forbid`。时间戳用 `now_unix_seconds()`。

## 交付物

### 1. `crates/agentflow-core/src/argument.rs`（扩展，新增判决投影）

H1 已把判决写进 `argument.verdict_rendered` 事件（payload 含 `hypothesis_id` / `verdict`（文本 `affirmed`|`refuted`|`inconclusive_provisional`|`inconclusive_fundamental`）/ `confidence` / `rationale`）。新增**只读投影**供 H2 读回粗粒度判决：

```rust
pub enum VerdictTag { Affirmed, Refuted, InconclusiveProvisional, InconclusiveFundamental }
// as_str / parse：parse 接受上述 4 个文本（与 verdict_payload_text 产出一致）

pub struct VerdictSummary {
    pub hypothesis_id: String,
    pub tag: VerdictTag,
    pub confidence: Confidence,   // 复用 hypothesis::Confidence
    pub created_at: i64,
}

impl ProjectStore {
    /// 取某假设最近一次 verdict_rendered 事件并解析为粗粒度摘要；无判决返回 None。
    pub fn latest_verdict_for(&self, hypothesis_id: &str)
        -> Result<Option<VerdictSummary>, StorageError>;
}
```

> 注：`VerdictSummary` 只需 tag + confidence；H1 payload 未存 Provisional 的 `missing` / Fundamental 的 `frontier` 明细，分支选择也不需要，**不要**为此改 H1 的 payload 格式。

### 2. `crates/agentflow-core/src/branch.rs`（新建）

```rust
use crate::argument::VerdictTag;
use crate::hypothesis::{Confidence, HypothesisStatus};

pub enum CandidateKind { Deepen, Spawn, Abandon, Hold }
// 映射（无 verdict → Hold）：
//   Affirmed                 -> Spawn   （已支持，派生相关子研究）
//   Refuted                  -> Abandon （负结果，停止该分支）
//   InconclusiveProvisional  -> Deepen  （还没挖够，补证据）
//   InconclusiveFundamental  -> Abandon （本质不可判，标研究空白后停）
//   None(无判决)             -> Hold    （需先出判决）

pub struct BranchCandidate {
    pub hypothesis_id: String,
    pub statement: String,
    pub verdict: Option<VerdictTag>,
    pub confidence: Option<Confidence>,
    pub kind: CandidateKind,
    pub evidence_count: usize,
    pub score: i32,            // 确定性 exploit 评分
}

pub struct BranchPolicy { pub explore_enabled: bool }
// false => 纯 exploit（按 score 降序，确定性）
// true  => 把「最欠探索」的候选（evidence_count 最小，并列取 created_at 最早）提到首位，标记为 Explore

pub enum SelectionMode { Exploit, Explore }

pub enum BranchAction {
    Deepen  { reason: String },
    Spawn   { reason: String },
    Abandon { reason: String, recommend_status: HypothesisStatus }, // 仅建议，不应用
    Hold    { reason: String },
}

pub struct BranchDecision {
    pub candidate: BranchCandidate,
    pub action: BranchAction,
    pub selected_by: SelectionMode,
}

pub trait BranchSelector {
    /// 确定性排序：按 score 降序，并列按 created_at/hypothesis_id 稳定排序。
    fn rank(&self, candidates: Vec<BranchCandidate>) -> Vec<BranchCandidate>;
}
pub struct RuleBasedSelector;
```

确定性评分（模块常量）：
- 基分按 kind：`Spawn=40, Deepen=30, Abandon=10, Hold=0`
- 置信度加分：`High=+6, Medium=+3, Low=+1, None=0`
- 排序并列稳定 tie-break：`hypothesis_id` 升序（保证测试可复现）。

`BranchAction` 派生（reason 为确定性说明串）：
- Deepen/Spawn/Hold reason 描述对应 kind；
- Abandon 的 `recommend_status`：来自 `Refuted` → `Contradicted`；来自 `InconclusiveFundamental` → `Superseded`。

`impl ProjectStore`：
- `branch_candidates(&self) -> Result<Vec<BranchCandidate>, StorageError>`
  - 对每个 `list_hypotheses` 的假设：`latest_verdict_for` + `evidence_for(...).len()` → 派生 kind、评分，组装候选。
- `select_branches(&self, selector: &dyn BranchSelector, policy: &BranchPolicy) -> Result<Vec<BranchDecision>, StorageError>`
  - 取候选 → `selector.rank` → 按 policy（exploit 直接用排序；explore 提升一个最欠探索者到首位并标 Explore）→ 每个映射成 `BranchDecision`。
- `propose_branch_patch(&self, flow_id: &str, decision: &BranchDecision, step: &ProposedStep) -> Result<GraphPatchRecord, StorageError>`
  - **仅** `Deepen`/`Spawn` 有效；`Abandon`/`Hold` → `StorageError::InvalidInput`（"abandon/hold 不产出图变更：abandon 是需用户决策的建议"）。
  - 把 `step` 序列化成 `{"ops":[{"op":"add_step","id":..,"tool":..,"needs":[..],"inputs":{..},"params":{..},"outputs":{..}}]}`，调用现有 `propose_graph_patch(flow_id, title, reason, patch_json)`。title 形如 `"branch:deepen <hyp_id>"`，reason 用 decision 的 action.reason。
  - `ProposedStep`（caller 提供，未来由 H6 觅食/选工具填充；测试给假值）：
    ```rust
    pub struct ProposedStep {
        pub id: String, pub tool: String,
        pub needs: Vec<String>,
        pub inputs: Vec<(String, String)>,
        pub params: Vec<(String, String)>,
        pub outputs: Vec<(String, String)>,
    }
    ```

### 3. `crates/agentflow-core/src/lib.rs`

新增 `pub mod branch;`。

## 验收标准（Claude 审核时逐条核对）

- [ ] clippy `-D warnings` 无警告；`cargo test --workspace` 全绿，core 测试数较 109 净增。
- [ ] 无新增依赖（4 个 Cargo.toml 零变更）；无新数据库表；无新增 event_type（图变更复用 `propose_graph_patch`）。
- [ ] verdict→kind 五种映射各有测试；无 verdict → Hold。
- [ ] 评分与排序确定性，有测试断言具体顺序。
- [ ] `explore_enabled=true` 把最欠探索候选提首位并标 `Explore`，有测试。
- [ ] `propose_branch_patch`：Deepen/Spawn 成功产出 `add_step` 提议且 patch_json 可被现有 graph_patch 解析；Abandon/Hold 被拒。
- [ ] **未调用** `apply_graph_patch` / `transition_hypothesis`（grep 确认 branch.rs 不含这两个调用）。

## 不在本里程碑（明确排除）

自动应用图变更（→ H7）、真随机 ε（→ H7）、工具发现/选择（→ H6）、CLI、防自欺闸门、置信门槛可配。
