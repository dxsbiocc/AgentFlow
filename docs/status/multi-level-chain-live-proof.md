# 验证记录:多层自治链 live 证明(agent 自建 3 步 producer ladder)

Date: 2026-06-24
Status: PASS — `agent run` 从一个假设 + **只有 RawCounts**(无 NormalizedCounts、无 ExpressionTable)+ SurvivalTable 出发,**递归**反向链出**两级 producer**,接成一条 3 步流(normalize → log2 → survival),按依赖顺序跑完,产出真实证据,诚实交接。全程 0-LLM。

## 动机

#94 证明了**一层**反向链(producer→consumer)。本次把它推到**多层**:当某个 producer 自己的输入也不存在时,再往下链一层 producer——这样 agent 能自建任意深度(到上限)的分析图。这正是 OmicOS 说的"组合不同算法挖尽数据维度",用我们的方式(每工具隔离 + 可审计)做。

## 机制

`chain_producer_steps` 改为递归(`chain_producer_steps_rec`):
- 对一个步骤每个 `artifact_REPLACE_<name>` 占位输入,按 `match_tools(desired_output_type=<类型>)` 取候选(分数排序,**High-fit producer(输入全可用)优先**,能终止该分支)。
- 选中候选后 `draft_step_for` 起草,**递归**满足它自己的占位输入(depth+1)。
- **全有或全无 grounding**:只有当 producer(及其子链)所有输入都不再是占位符时才提交;否则回滚(还原 visited 快照),保留占位符 → apply 阶段优雅失败(同 #94)。
- 深度上限 `MAX_CHAIN_DEPTH=4` + 路径 `visited` 集防环。返回顺序 = **最深的 producer 在前**,每个都排在消费它的步骤之前。

并把等价分支刹车的排除做成**传递闭包**:`has_equivalent_tool_branches` 先把 top 候选的**完整链**建出来,收集链上所有"消费但不可用"的中间类型(本例 {ExpressionTable, NormalizedCounts}),凡是产出其中任一类型的候选都排除——它是链的**补集**不是替代答案。真有第二个答案工具(产出最终 observation 类型)时,刹车照常触发。

## live 结果(agent 自行跑,我只分析)

输入:假设「SPP1 expression associates with overall survival in the imported cohort」+ 导入 **RawCounts** + SurvivalTable。注册三件工具:`local/normalize_counts`(RawCounts→NormalizedCounts,CPM)、`local/normalized_to_expression`(NormalizedCounts→ExpressionTable,log2)、`local/survival_assoc`(ExpressionTable+SurvivalTable→report)。**未导入** NormalizedCounts / ExpressionTable;**未注册** counts_to_expression(否则一层 High-fit 链会短路)。

```
Applied:
  lifecycle -> under_test
  flow auto_event_... auto-created
  step step_normalize_counts ran without observation            ← 最深 producer(CPM)
  graph patch ... step step_normalized_to_expression
  step step_normalized_to_expression ran without observation    ← 中间 producer(log2)
  graph patch ... step step_survival_assoc
  step step_survival_assoc ran and observed observation_...      ← consumer(median-split log-rank)
Outcome: handed_off
Decision: [stance_assessment] ... SPP1 score 4.353 ... 请判定立场
⚠ 该结果依赖推断的未确认参数:gene=SPP1
```

- flow 结构:**Steps: 3, Edges: 2**(normalize → log2 → survival 一条线)。counts:flows 1 / steps 3 / runs 3 / artifacts 5。
- 三步按依赖顺序跑完;consumer 分析的是两级 producer 处理后的 ExpressionTable。
- **score 4.353 ≠ 参考 4.306**——诚实:CPM 归一改变了样本间排序,这是一条**不同的**(CPM→log2→survival)管线,不是复现参考。这恰好证明数据真的流过了整条链(而非旁路)。
- **未自治 affirm**:raise StanceAssessment 交人 + 提示确认推断参数。诚实不变量守住。

## 结论

- **多层自治链 live 证明**:假设 → 自主匹配 consumer → 递归发现两级输入缺口 → 自主链出两级 producer → 自建 3 步流 → 按序跑真分析 → 真实证据 → 诚实交接。
- 等价分支刹车的传递闭包排除:整条链的 producer 都被识别为补集,不再误触发;真有替代答案时仍正确刹车。
- 回归测试 `crates/agentflow-cli/tests/multi_step_chain.rs::agent_backward_chains_multiple_producer_levels` 锁定(RawCounts→MidCounts→ExpressionTable 两级梯子,断言 Steps:3/Edges:2)。

## 边界(诚实)

- 深度上限 4;超过则该分支不 grounding → apply 优雅失败。
- producer 选择仍偏好 High-fit、再退而求其次递归;`match_tools` 仍可能让强关键词 producer 抢 top(answer-vs-intermediate 排序是后续)。
- **执行仍是顺序**:`run_flow_with` 按调度顺序串行跑 ready 步骤(共享单个 SQLite 连接)。真正的**并行执行**需要连接池 + 线程安全,是一笔独立的并发改造,本次有意未做(见 [[no-high-load-local-builds]] 风险)。本次交付的是"更深的图",不是"并行跑图"。
