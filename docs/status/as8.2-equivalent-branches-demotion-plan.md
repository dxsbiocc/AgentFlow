# AS8.2 实现简报：等价分支判定与 question-aware fit 对齐（修复 AS8.1 之后仍不可达的 A3 刹车）

Status: Assigned to Codex（继续 feat/question-aware-fundamental 分支，AS7+AS8+AS8.1 之上新增提交）
Owner: Claude(编排) · Codex(执行)
Spec source: 编排者 live 实证后定位的根因修复——AS8.1 落地并 live 验证生效后，A3 `DeepenOrStop` 刹车仍每轮重复触发，`auto_synth`/AS7/AS8 路径仍不可达
Depends on: AS1-AS8.1（本分支已有 AS7 5649728 + AS8 4682abc + AS8.1 e2fc9a6，均 core 279 / cli 92 绿）

## 背景（live 实证 + 根因定位）

AS8.1 落地并 live 验证：免疫治疗假设（`event_1780637981901936000`）的顶部候选工具（生存关联代理）正确被 question-aware scorer 判「答不了这个具体问题」，`matched_fit` 从 medium 降为 `low`，reason 含 `relevance:demoted_question_mismatch`，`auto_synth_gap(&proposal)` 变为 `true`——AS8.1 设计目标达成。

但 live 跑两轮后，`auto_synth`/AS7/AS8 仍从未执行：每轮都在同一个 `DeepenOrStop`（"已产生可落地步骤...自动应用前触发刹车"）decision 上打转，`applied`/`apply_failures`/`source_discoveries` 始终为 `null`。

根因：`enrich_branch_proposal`（`agent.rs:884`）内部调用 `apply_semantic_relevance_to_candidates` 对候选做了 question-aware 降级，但 `has_equivalent_tool_branches`（`agent.rs:1014`）**自己重新跑了一次原始 `match_tools`**，不经过任何 question-aware 降级：

```rust
fn has_equivalent_tool_branches(
    &self,
    decision: &BranchDecision,
    available_input_types: &[String],
) -> Result<bool, StorageError> {
    let query = CapabilityQuery {
        desired_output_type: None,
        available_input_types: available_input_types.to_vec(),
        keywords: proposal_keywords(&decision.candidate.statement),
    };
    let candidate_count = self
        .match_tools(&query)?
        .into_iter()
        .filter(|candidate| matches!(candidate.fit, Fit::High | Fit::Medium))
        .take(2)
        .count();
    Ok(candidate_count > 1)
}
```

对免疫治疗假设，原始 `match_tools` 返回 ≥2 个因 `relevance:keyword` 被 TM1 机械促升为 `Fit::Medium` 的候选（生存代理工具 + 轴关联工具等）。`has_equivalent_tool_branches` 据此认为「存在 ≥2 个等价分支」→ `equivalent_branches=true` → `DefaultPolicy::assess`（`handoff.rs:211`）命中 `!ctx.reversible || ctx.equivalent_branches` 分支 → 返回 `Some(DeepenOrStop)` → 在 `run_cycle_with_scorer`（`agent.rs:425-477`）的 `auto_synth` 判定块中，这个刹车在 `auto_synth_gap(&proposal)` 为真之后、但**早于** `synthesizer.synthesize`/AS7 源发现/AS8 FundamentalGap 的实际执行——刹车每轮以同一 digest 重新触发，与是否解决上一轮 decision 无关。

**核心修复点**：`has_equivalent_tool_branches` 必须使用与 `enrich_branch_proposal` 同一份「question-aware 降级后」的候选集合来判定"等价分支"，而不是重新跑一次原始 `match_tools`。两处对"这个假设有几个真正能用的工具"的判断必须口径一致。

## 编排者裁决（约束）

1. **`has_equivalent_tool_branches` 增加 `scorer: &dyn RelevanceScorer` 参数**（`agent.rs:1014`）：
   - 函数体内 `match_tools` 之后，调用同一个 `apply_semantic_relevance_to_candidates(self, &mut candidates, &decision.candidate.statement, scorer)?`（与 `enrich_branch_proposal` 内部完全一致的调用）。
   - 降级/促升后，再 `.filter(|candidate| matches!(candidate.fit, Fit::High | Fit::Medium))` 计数，逻辑其余部分不变（`take(2)`、`count() > 1`）。

2. **更新两处调用方**（`run_cycle_with_scorer`，`agent.rs` 约 471 行与约 642 行）：
   - 两处 `self.has_equivalent_tool_branches(&proposal.decision, &available_input_types)?` 改为 `self.has_equivalent_tool_branches(&proposal.decision, &available_input_types, scorer)?`。
   - `scorer: &dyn RelevanceScorer` 在该函数作用域内已是入参（第 459 行已使用），直接传入即可，无需新增参数穿透。

3. **不改 `apply_semantic_relevance_to_candidates` / `auto_synth_gap` / `argument.rs`**：本次只对齐"等价分支计数"与"顶部候选 fit 判定"使用同一降级口径，不改变降级算法本身。验证 `argument.rs` 仍 0 处 LLM/网络调用。

4. **预期行为变化（不是回归）**：
   - 对免疫治疗假设场景：`apply_semantic_relevance_to_candidates` 在 `has_equivalent_tool_branches` 内部对同一组候选做相同降级后，原本 ≥2 个 `relevance:keyword` 促升的 Medium 候选中，凡是 question-aware scorer 判 `Some(false)` 的都会降为 Low；若降级后 `Fit::High|Medium` 候选数 ≤1，则 `equivalent_branches=false`，A3 刹车不再因此触发，`auto_synth`/AS7/AS8 分支得以继续执行（synthesize/reuse/source-discovery）。
   - 若降级后仍有 ≥2 个 `Fit::High|Medium` 候选（例如多个工具都能直接回答该问题），`equivalent_branches=true` 保持不变，刹车按设计正常触发——这是合理行为，不应被绕过。

5. **测试（离线 stub，覆盖核心场景）**：
   - `agent.rs` 新增/扩展测试（用 `StubRelevanceScorer`、复用/扩展 `setup_keyword_relevance_project` 或新增类似 helper，注册 ≥2 个因 `relevance:keyword` 促升为 Medium 的工具）：
     a. ≥2 个候选均因纯关键词促升为 Medium，stub 对全部返回 `Some(false)` → `has_equivalent_tool_branches` 降级后 `Fit::High|Medium` 候选数 ≤1 → 返回 `false`；端到端 `run_cycle_with_synth`：`equivalent_branches=false`，不触发 `DeepenOrStop`，`auto_synth_gap` 路径正常执行（synthesizer 被调用 / `apply_failures` 含 "auto-synth skipped" 等既有 AS8 断言可复用）。
     b. 同样 ≥2 候选场景，stub 对全部返回 `Some(true)`（确实都能直接回答）→ 降级后仍 ≥2 个 Medium/High → `has_equivalent_tool_branches` 返回 `true` → `equivalent_branches=true` → `DeepenOrStop` 刹车按设计触发（防回归：等价分支检测本身不应被破坏）。
     c. 单候选场景（防回归）：只有 1 个工具匹配，`has_equivalent_tool_branches` 在加入 scorer 参数前后行为一致，仍返回 `false`。
   - AS1-AS8.1 既有测试全部保持绿（核心是 capped_evidence_grade / FundamentalGap / 自治合成链 / AS8.1 demotion 测试不回归）。
   - core 测试数预期从 ~279 增至 ~281-283（新增 2-3 个）。

6. **保留 AS1-AS8.1 全部既有保证**：no-fabrication、输入敏感性、运行时门、测试-修复、grounding、验证客户端复用、安全 allowlist、L4 ⚠ 未验证、0 判决权重、默认全开、可配置 LLM、dedup、AS7 安全 allowlist、AS8 FundamentalGap+人类确认、AS8.1 question-aware fit 降级。

7. core 零 LLM 依赖（`apply_semantic_relevance_to_candidates` 调用的 `scorer` 走既有 `RelevanceScorer` trait 抽象，stub 可离线测试）；无新 Rust 依赖；签名变更属于内部私有方法（`fn has_equivalent_tool_branches`，非 `pub`），不影响外部 API/事件 payload 结构。

## 交付物

- `crates/agentflow-core/src/agent.rs`：
  - `has_equivalent_tool_branches` 增加 `scorer: &dyn RelevanceScorer` 参数，内部对候选应用 `apply_semantic_relevance_to_candidates` 后再计数。
  - `run_cycle_with_scorer` 中两处调用方传入 `scorer`。
- 测试：上述 a/b/c 三类，离线 stub，AS1-AS8.1 全部保持绿。

## 验收标准（Claude 复核 + live）

- [ ] clippy/test/acceptance/fmt 全绿；AS1-AS8.1 加固与测试保持（core ≥279 不减少，新增覆盖本次修复）。
- [ ] 单测证明：≥2 个纯关键词促升 Medium 候选、question-aware scorer 全部判「答不了」→ 降级后等价分支数 ≤1 → `equivalent_branches=false` → 不触发 `DeepenOrStop` → auto_synth 路径继续执行。
- [ ] 单测证明：≥2 个候选 question-aware scorer 判「确实能回答」→ 等价分支数仍 ≥2 → `equivalent_branches=true` → `DeepenOrStop` 按设计触发（防回归）。
- [ ] `argument.rs` 仍 0 处 LLM/网络调用（`render_verdict` 唯一判决出口不变）。
- [ ] **live（编排者，真 DeepSeek+网络，遵守"不干预，自行处理"规则）**：纯 `agent run` 跑 MID1IP1 免疫治疗问题 → 不再每轮重复同一个 `DeepenOrStop`（"自动应用前触发刹车"）decision → `auto_synth`/AS7 源发现 实际执行（`source_discoveries`/`apply_failures` 不再恒为 null）→ 若无 viable 源 → AS8 raise FundamentalGap，digest 诚实呈现"这是研究空白"。
- [ ] core 测试数不减少（预期 ~281-283）；无新依赖。

## 不在本里程碑

- 不改 `argument.rs` 判决逻辑、不改 `DecisionKind`/事件结构；不引入新的 LLM trait；不改 AS7 安全 allowlist；不改 AS8.1 的降级算法本身；不做通用搜索引擎集成。
