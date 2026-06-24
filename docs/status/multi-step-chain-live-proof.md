# 验证记录:多步自治链 live 证明(agent 自建 producer→consumer 两步流)

Date: 2026-06-23
Status: PASS — `agent run` 从一个假设 + 只有 RawCounts(无 ExpressionTable)出发,**自主**反向链(backward-chain)出一个 producer 步骤来满足 consumer 缺失的输入类型,把两步接成一条流、按依赖顺序跑完、产出真实证据,并诚实交接。全程 0-LLM。

## 动机

单步自治闭环(#93)、人写的两步组合(#74)、调度器(#76)、needs 接线(#77)各自验过。缺的一环:**agent 自己把多步接起来**——当匹配到的 consumer 需要一个当前不存在的输入类型时,agent 能不能自己找一个 producer 来补上。这正是 OmicOS 说的"第二层"(组合不同算法),用我们的方式(每工具隔离 + 可审计)做。

## 机制

`draft_step_for` 对没有可用工件满足的必需输入,会填 `artifact_REPLACE_<name>` 占位符(apply 校验会失败)。新增 `chain_producer_steps`:对每个这样的占位符输入,用 `match_tools(desired_output_type=<该类型>)` 找一个 **High-fit** 的 producer(输出该类型、且它自己的输入全部可用),起草该 producer 步骤,把 consumer 的输入从占位符改写成 `<producer_step>.<output>`。`EnrichedProposal` 多带一个 `prerequisite_steps`(空时不上 JSON 线,旧字节不变)。apply 阶段先按序 apply+run 每个 producer(producer 先建流,consumer 再 patch 进同一条流),再 apply+run consumer;`run_step_ref` 的 `ensure_step_dependencies_completed` 保证顺序正确。

并修了一个交互 bug:`has_equivalent_tool_branches`(等价分支刹车)会把 producer 误判成 consumer 的"替代答案"从而触发刹车、丢掉 prerequisites。修正:排除掉"产出 top 候选缺失输入类型"的候选——producer 是**补集**不是**替代**,不计入等价分支。真正有两个候选答案时刹车照常触发。

## live 结果(agent 自行跑,我只分析)

输入:假设「SPP1 expression associates with overall survival in the imported cohort」+ 导入 **RawCounts**(由 expression 取整成整数伪计数,365 样本)+ SurvivalTable。注册 producer `local/counts_to_expression`(RawCounts→ExpressionTable,log2(x+1))+ consumer `local/survival_assoc`。**没有导入任何 ExpressionTable**。

```
Applied:
  lifecycle -> under_test
  flow auto_event_... auto-created
  step step_counts_to_expression ran without observation      ← producer 跑了(log2 归一),无 observation(中间步,正确)
  graph patch ... applied ... step step_survival_assoc         ← consumer patch 进同一条流
  step step_survival_assoc ran and observed observation_...     ← consumer 跑了(median-split log-rank)
Outcome: handed_off
Decision: [stance_assessment] ... SPP1 score 4.306 ... 请判定立场
⚠ 该结果依赖推断的未确认参数:gene=SPP1
```

- `agent run --json` 证实起草内容:consumer 的 `expression_table` 输入被改写成 `step_counts_to_expression.expression`,`prerequisite_steps=[counts_to_expression(counts←RawCounts 工件)]`。
- flow 结构:**Steps: 2, Edges: 1**(producer→consumer 一条边)。counts:flows 1 / steps 2 / runs 2 / artifacts 4。
- producer 先跑、consumer 后跑;consumer 分析的是 producer 的 log2 输出。因 median-split 基于秩、log2 单调,链式管线**复现** SPP1 参考(score 4.306)——证明数据真的流过了整条链。
- **未自治 affirm**:raise StanceAssessment 交人判定立场 + 提示确认推断参数。诚实不变量守住。

## 结论

- **多步自治链 live 证明**:假设 → 自主匹配 consumer → 发现输入类型缺口 → 自主反向链出 producer → 自建两步流 → 按序跑真分析 → 真实证据 → 诚实交接。
- 等价分支刹车修正:producer/consumer 是互补关系,不再被误判为歧义;真有多个候选答案时仍正确刹车。
- 回归测试 `crates/agentflow-cli/tests/multi_step_chain.rs` 锁定:自包含 fixture(producer RawCounts→ExpressionTable + consumer),断言两步都跑、consumer observed、无 missing、Steps:2/Edges:1。

## 边界(诚实)

- 反向链**一层**:不会为 producer 自己缺失的输入再链一层 producer(避免递归;有用例再扩)。
- producer 选择要求 **High-fit**(输出目标类型且自身输入全可用),保证 producer 能直接跑。
- `match_tools` 仍可能把强关键词命中的 producer 排到 top(degenerate 分支);本次靠 consumer 关键词更强而 top 命中 consumer。更鲁棒的"答案 vs 中间产物"排序是后续。
