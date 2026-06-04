# TM1 实现简报：关键词相关性纳入 fit（修审计 🟡 A2，分阶第一步）

Status: Assigned to Codex
Owner: Claude(编排) · Codex(执行)
Spec source: 深度审计 🟡 A2 —— 用户选定「分阶：先确定性关键词入 fit」
Depends on: tool_select.rs match_tools —— 在 main

## 背景（缺口）
`fit` 判定纯由结构 I/O（output_match + 必填输入满足），**完全无视关键词相关性**。tcga 工具对 THRSP 假设命中 survival/expression 等关键词却仍 fit=Low（无 desired_output、无必填输入）。已有的关键词信号对 fit 不可见，削弱 exploit/explore 与等价分支判定质量。

## 目标
让确定性关键词相关性影响 fit：结构 I/O 不满足但关键词强相关时，fit 从 Low 提到 Medium。High 仍只保留给真正 I/O 匹配（能产出所需）。纯确定性、零依赖。

## 编排者裁决（约束）
1. **统计关键词命中**：在 match_tools 现有关键词扫描里，统计**去重**的命中数：name_kw=命中工具 name 的不同 query 关键词数，desc_kw=命中 description 的不同 query 关键词数（已有 reasons 里的 keyword:name:/keyword:description: 即此信号，去重计数即可）。
2. **强相关判定**：`strong_keyword_relevance = name_kw >= 1 || desc_kw >= 2`（命中工具名=强信号；≥2 描述词也算；单个描述词=弱，不提）。
3. **fit 增一档**（在 Low 之前）：
   ```
   High   = output_match && all_required_inputs_satisfied        （不变）
   Medium = output_match || majority_required_inputs_satisfied   （不变）
   Medium = strong_keyword_relevance                             （新增：原本会 Low 的提到 Medium）
   Low    = 其余
   ```
   **High 绝不由关键词触发**（保留给真正能产出所需的 I/O 匹配）。
4. **可见**：fit 因关键词被提升时，reasons 追加一项如 `relevance:keyword`（让"为何 Medium"可解释）。
5. **score 不变**：关键词仍按现有 SCORE_KEYWORD_* 加分；本步只改 fit 档位判定，不改 score 公式、不改排序逻辑。
6. **零回归**：仅 tool_select.rs match_tools 的 fit 判定 + 计数；现有测试中因「关键词强相关」从 Low 合理变 Medium 的断言相应更新（语义正确的演进），其它不变。无新依赖/表。

## 交付物
- tool_select.rs：match_tools 内 name_kw/desc_kw 去重计数 + strong_keyword_relevance + fit 增档 + reason。
- 测试：
  - 命中工具名关键词、无 I/O 匹配 → Medium（非 Low）（tcga/THRSP 类）。
  - 仅 1 个描述关键词命中、无 I/O → 仍 Low（弱不提）。
  - ≥2 描述关键词命中 → Medium。
  - output_match/输入满足的 High/Medium 路径不变。
  - 现有 fit 断言按新语义更新（仅必要处）。

## 验收标准（Claude 逐条复核）
- [ ] clippy -D warnings 干净；cargo test 全绿；acceptance 通过；fmt 通过。
- [ ] tcga 类（名含 survival）对 THRSP 假设 fit=Medium（行为/单元测试）。
- [ ] High 不被关键词触发（断言：纯关键词强相关但无 I/O → 至多 Medium）。
- [ ] 弱相关（单描述词）仍 Low。
- [ ] 零回归：score/排序不变；仅 tool_select.rs；无新依赖。

## 不在本里程碑
- LLM 语义评分缝（RelevanceScorer trait，分阶第二步，后续）。
- name vs description 更细权重调参（v1 用 name>=1 || desc>=2 简明规则）。

## 验收记录（Claude 独立复验 + live 2026-06-04）
- ✅ clippy/test(core 260,+4)/acceptance/fmt 全绿；仅 tool_select.rs；无新依赖/表；score 公式与排序未改。
- ✅ **live 实证**：tcga/survival_assoc 对 THRSP 假设 fit=medium（审计时 low），reason 含 relevance:keyword（"survival" 命中工具名 → strong_keyword_relevance）。
- ✅ High 不被关键词触发（纯关键词强相关无 I/O 至多 Medium，有测试）；弱相关（单描述词）仍 Low；≥2 描述词→Medium。
- ✅ 现有 fit 断言无需改（纯附加提升）。
结论：合并就绪。审计 🟡 A2 第一步闭合——确定性关键词相关性纳入 fit，相关工具不再被误判 Low，改善 exploit/explore 与等价分支判定。LLM 语义评分缝（RelevanceScorer）为分阶第二步、可选后续。
