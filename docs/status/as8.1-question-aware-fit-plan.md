# AS8.1 实现简报：问题感知 fit/缺口判定（修复 AS8 不可达根因）

Status: Assigned to Codex（继续 feat/question-aware-fundamental 分支，AS7+AS8 之上新增提交）
Owner: Claude(编排) · Codex(执行)
Spec source: 编排者 live 实证后定位的根因修复——AS8 的 FundamentalGap 路径在 MID1IP1 免疫治疗场景下永远不可达
Depends on: AS1-AS8（本分支已有 AS7 5649728 + AS8 4682abc，均 core 276 / cli 92 绿）

## 背景（live 实证 + 根因定位）

live 跑「MID1IP1 是否可作为肝癌免疫治疗疗效预测因子」时，AS8 的问题感知/FundamentalGap 路径**从未触发**。根因：`auto_synth_gap`（`crates/agentflow-core/src/agent.rs:1711`）只在 `matched_fit == Low || None` 时触发；但生存关联工具 `tcga/survival_assoc` 在两层都被「主题相关」误判成 Medium，绕过了 `auto_synth_gap`：

1. **TM1（机械关键词，`tool_select.rs:125-135`）**：候选 fit 为 Medium 当 `name_kw>=1 || desc_kw>=2`（`relevance:keyword`），完全是字符串重叠，不管工具的输出是否回答了假设里的具体结论（生存 vs 免疫治疗响应）。
2. **TM2（LLM 语义，`agent.rs:775` `apply_semantic_relevance_to_candidates` + `agent_ops_commands.rs:663` `relevance_prompt`）**：只扫描仍是 `Fit::Low` 的候选（`if candidate.fit != Fit::Low { continue; }`），且 prompt 问的是「假设与工具是否**研究相关**」——这是主题相关性，不是「工具能否回答这个具体问题」。survival 工具和 MID1IP1/肝癌主题相关，LLM 自然答 yes。

结果：`tcga/survival_assoc` 被判 `matched_fit=medium`，`auto_synth_gap` 永远是 false，AS7 源发现 / AS8 研究空白判定**整条链都进不去**——生存关联结果被当成「答案」直接走 `stance_assessment`。

**核心修复点**：把"主题相关 (topical relevance)"和"能否直接回答该假设的具体结论 (question-answering capability)"分开。TM1/TM2 的 Medium 促升只要是**纯主题相关**（无 output_match / 无多数必需输入满足），就必须再过一道「问题感知」LLM 检查；答不了就降回 Low，让 `auto_synth_gap` 正确触发。

## 编排者裁决（约束）

1. **重新定义 relevance 语义为「question-answering」而非「topical」**：
   - 修改 `crates/agentflow-cli/src/agent_ops_commands.rs` 的 `relevance_prompt`（`agent_ops_commands.rs:663`），从「假设「{statement}」与工具 <{tool_ref}>（描述：{tool_description}）是否研究相关？只答 yes/no。」改为问「该工具的输出能否**直接**作为证据来检验这个假设里陈述的具体结论（而不只是主题/疾病/基因相关）？只答 yes/no。」
   - `RelevanceScorer` trait 本身（`agent.rs:95`）签名不变，仅语义文档注释更新（说明现在是 question-answering 而非 topical）。

2. **扩大 TM2 的复核范围到「纯关键词促升」的 Medium 候选**：
   - `apply_semantic_relevance_to_candidates`（`agent.rs:775`）当前只处理 `candidate.fit == Fit::Low`。新增逻辑：对 top-K 内 `fit == Fit::Medium` 且 `reason` 包含 `"relevance:keyword"`（即 TM1 纯关键词促升，无 `output_match`/多数必需输入满足——这是 `tool_select.rs` 中唯一推送 `"relevance:keyword"` 的分支，结构性匹配不会有这个标记）的候选，调用同一个（已改为 question-aware 的）scorer：
     - `Some(false)` → 降回 `Fit::Low`，reason 追加新常量（如 `"relevance:demoted_question_mismatch"`）。
     - `Some(true)` 或 `None` → 维持 Medium，不变。
   - 仍保留原有 Low→Medium 促升逻辑（`Some(true)` 时促升，reason 追加 `relevance:semantic`），其语义现在天然是「question-aware」。
   - 促升或降级后都需要重新排序（复用现有 sort 逻辑）。

3. **不改 `auto_synth_gap` / `argument.rs`**：降级后候选变 Low，`enrich_branch_proposal`（`agent.rs:868`）的 `top = candidates.into_iter().next()` 与现有 `auto_synth_gap` 检查（`matched_fit == Some("low")`）天然正确触发，无需改动。验证 `argument.rs` 仍 0 处 LLM/网络调用。

4. **测试（离线 stub，覆盖核心场景）**：
   - `tool_select.rs`：保持现有 TM1 测试不变（`match_tools_promotes_name_keyword_relevance_without_io_match_to_medium` 等）——TM1 本身的 fit 计算不变，只是 agent.rs 多一道复核。
   - `agent.rs` 新增/扩展测试（用 `StubRelevanceScorer`）：
     a. 一个工具因 `relevance:keyword` 促升为 Medium，但 stub 对该工具返回 `Some(false)`（"相关但答不了这个问题"）→ 复核后降回 Low → `auto_synth_gap` 为 true → 走 AS7/AS8 路径（可复用 AS8 已有的 source-discovery/FundamentalGap 测试断言其被触发）。
     b. 同样场景但 stub 返回 `Some(true)`（"确实能直接回答"）→ 维持 Medium，`auto_synth_gap` 为 false，行为与现状一致（防回归）。
     c. `Fit::Low` 候选经 question-aware scorer 返回 `Some(true)` → 仍按现有逻辑促升为 Medium（防回归 TM2 原有路径）。
     d. AS1-AS8 既有测试全部保持绿（核心是 capped_evidence_grade / FundamentalGap / 自治合成链不回归）。
   - core 测试数预期从 276 增至 ~279-281（新增 2-4 个）。

5. **digest/文案**：若 `relevance:demoted_question_mismatch` 这类新 reason 字符串出现在任何面向用户的 digest 中，确保措辞中性、不暴露内部实现细节给最终用户（内部 reason 字段本身允许保留英文 slug，仅 L4/AS8 digest 的人类可读文本需要中文且诚实）。

6. **保留 AS1-AS8 全部既有保证**：no-fabrication、输入敏感性、运行时门、测试-修复、grounding、验证客户端复用、安全 allowlist、L4 ⚠ 未验证、0 判决权重、默认全开、可配置 LLM、dedup、AS7 安全 allowlist、AS8 FundamentalGap+人类确认。

7. core 零 LLM 依赖；无新 Rust 依赖；新增字符串常量向后兼容（不改变已有事件/decision payload 的字段结构，只新增 reason 文本）。

## 交付物

- `crates/agentflow-cli/src/agent_ops_commands.rs`：`relevance_prompt` 改为 question-answering 语义。
- `crates/agentflow-core/src/agent.rs`：`apply_semantic_relevance_to_candidates` 扩展，复核「纯关键词促升的 Medium」候选，必要时降级为 Low；新增 reason 常量。
- 测试：上述 a/b/c/d 四类，离线 stub，AS1-AS8 全部保持绿。

## 验收标准（Claude 复核 + live）

- [ ] clippy/test/acceptance/fmt 全绿；AS1-AS8 加固与测试保持（core ≥276 不减少，新增覆盖本次修复）。
- [ ] 单测证明：纯关键词促升的 Medium 候选，若 question-aware scorer 判「答不了这个具体问题」→ 降回 Low → `auto_synth_gap` 触发。
- [ ] 单测证明：question-aware scorer 判「能直接回答」时（Low→Medium 促升 或 Medium 维持）行为不回归。
- [ ] `argument.rs` 仍 0 处 LLM/网络调用（`render_verdict` 唯一判决出口不变）。
- [ ] **live（编排者，真 DeepSeek+网络，遵守"不干预，自行处理"规则）**：纯 `agent run` 跑 MID1IP1 免疫治疗问题 → `tcga/survival_assoc`（或同类生存代理）因"答不了免疫治疗响应"被 question-aware scorer 判 `Some(false)` → 降回 Low → `auto_synth_gap` 触发 → 走 AS7 源发现 → 若无 viable 源 → AS8 raise FundamentalGap，digest 诚实呈现"这是研究空白"。
- [ ] core 测试数不减少（预期 ~279-281）；无新依赖。

## 不在本里程碑

- 不改 `argument.rs` 判决逻辑、不改 `DecisionKind`/事件结构；不引入新的 LLM trait；不改 AS7 安全 allowlist；不做通用搜索引擎集成。
