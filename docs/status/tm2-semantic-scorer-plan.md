# TM2 实现简报：LLM 语义相关性缝 RelevanceScorer（修审计 🟡 A2 第二步）

Status: Assigned to Codex
Owner: Claude(编排) · Codex(执行)
Spec source: 深度审计 🟡 A2 第二步 —— 用户选定「分阶」，本步加 LLM 语义缝
Depends on: TM1（关键词入 fit）、L2（ParamInferer 缝样板）、synth_commands.run_synthesizer —— 均在 main

## 背景
TM1 让确定性关键词相关性入 fit，但仍需词面重叠。LLM 语义缝捕捉**无词面重叠的相关性**（如 THRSP↔survival 机制相关）。core 零依赖铁律：core 定 trait 缝，LLM 实现走 CLI 子进程（镜像 ParamInferer）。判决引擎绝不碰。

## 目标
加 `RelevanceScorer` trait（core，dep-free）；run_cycle 可选用它在 enrich 时把语义相关但结构 Low 的候选提到 Medium。默认 Noop = TM1 行为不变。CLI 出 LLM 实现，`--semantic-match` opt-in。

## 编排者裁决（约束）
1. **core trait + Noop**（镜像 ParamInferer，agent.rs）：
   `pub trait RelevanceScorer { fn is_relevant(&self, hypothesis_statement: &str, tool_ref: &str, tool_description: &str) -> Option<bool>; }`
   `NoopRelevanceScorer` → 恒 None。
2. **缝接入**：新增 `run_cycle_with_scorer(config, inferer: &dyn ParamInferer, scorer: &dyn RelevanceScorer)`；现有 `run_cycle_with(config, inferer)` 委托 `&NoopRelevanceScorer`（保持现有调用点/签名链，零回归）。
3. **语义提升（enrich_branch_proposal，agent.rs:471）**：match_tools 得到排序候选后，对 **top-K=3** 候选：若该候选 `fit == Low` 且 `scorer.is_relevant(...) == Some(true)` → 提到 `Fit::Medium`、reason 追加 `relevance:semantic`；随后按 (fit 降, score 降) 重排，再取 top 作 proposal。`has_equivalent_tool_branches`（line 528 的 match_tools）**保持确定性，不接 scorer**。
4. **High 不被语义触发**：语义只能 Low→Medium（与 TM1 同；High 保留给真 I/O 匹配）。
5. **CLI 实现 + opt-in**（agent_ops_commands.rs，镜像 LlmParamInferer）：`LlmRelevanceScorer{ synthesizer }` 用 `run_synthesizer` 发提示「假设 X 与工具 <ref>（描述：<desc>）是否研究相关？只答 yes/no」，解析 yes→Some(true)/no→Some(false)/失败→None。`agent run` 加 `--semantic-match`（需 --synthesizer，默认 claude -p）；不传则用 NoopRelevanceScorer（行为=TM1）。
6. **零回归**：默认 Noop → 现有全部测试不改且通过；判决/PV/L4/TM1 逻辑不变；core 无新依赖（LLM 在 CLI 子进程）；无新表/event_type。
7. **成本有界**：每 cycle 至多 K=3 次 scorer 调用（只对 Low-fit 候选调；Some(false)/None 不提）。

## 交付物
- agent.rs：RelevanceScorer trait + Noop；run_cycle_with_scorer + 委托链；enrich 语义提升+重排。
- agent_ops_commands.rs：LlmRelevanceScorer（run_synthesizer）；--semantic-match flag 接线。
- 测试（core，用 stub scorer）：stub 返回 true 把 Low 候选提 Medium 并影响 top 选择；stub None/false 不提；NoopRelevanceScorer 行为=TM1（现有测试不改）；High 不被语义触发；top-K 上限。

## 验收标准（Claude 复核 + live）
- [ ] clippy -D warnings 干净；cargo test 全绿；acceptance 通过；fmt 通过。
- [ ] stub scorer 行为测试（true 提升+重排 / false/None 不提 / High 不被触发 / K 上限）。
- [ ] 默认（无 --semantic-match）零回归：现有测试不改且通过；core 无新依赖。
- [ ] live（真 claude）：`agent run --semantic-match` 对一个无词面重叠但语义相关的工具/假设，语义提升生效（reason 含 relevance:semantic）。
- [ ] 判决/TM1/PV/L4 逻辑未改。

## 不在本里程碑
- 语义分用于 High（保留 I/O 锚定）。
- 全候选重排（仅 top-K=3 有界）。
- embedding 向量检索（子进程 LLM 足够，向量索引是更大后续）。

## 验收记录（Claude 独立复验 + live 2026-06-04）
- 说明：实现一度因 macOS TCC（Claude.app 自更新重置 Downloads 授权）在 TDD 红灯被打断；权限恢复后由 Codex 补完生产侧让已写测试转绿。
- ✅ clippy/test(core 266,+6)/cli 58/schemas 3 全绿；acceptance/fmt 通过；core 无新依赖。
- ✅ 生产侧：RelevanceScorer trait(92) + NoopRelevanceScorer(102) + run_cycle_with_scorer(236) + apply_semantic_relevance_to_candidates(488)；run_cycle_with 委托 Noop。
- ✅ **判决不变量**：argument.rs 零 diff——语义只调 fit、不碰 render_verdict/score/cap。
- ✅ apply_semantic：仅 fit==Low 候选、Some(true)→Medium+reason relevance:semantic、按(fit,score)重排；High 不被语义触发；top-K=3 有界。
- ✅ CLI：LlmRelevanceScorer(run_synthesizer，yes/no) + --semantic-match（需 --synthesizer，默认 claude -p）；不传走 Noop=TM1 行为。
- ✅ **live（真 claude）**：agent run --semantic-match 端到端跑通；tcga 已被 TM1 提 Medium，语义正确地只作用于 Low 候选未覆盖它。
结论：合并就绪。审计 🟡 A2 第二步（LLM 语义缝）闭合——语义相关性可把关键词漏掉的 Low 候选提到 Medium，opt-in、默认零回归、判决确定性不受影响、core 零依赖、LLM 走子进程缝。
