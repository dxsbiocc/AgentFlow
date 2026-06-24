# 验证记录:全自治闭环 live 证明(agent 自建流 + 真分析 + 诚实交接)

Date: 2026-06-23
Status: PASS — `agent run` 从一个假设出发,**自主**匹配工具、填参、建流、跑真分析、产出证据,并**诚实交接**(StanceAssessment + 未确认参数提示),全程**无 LLM**。

## 动机

组合(#74)、调度(#76)、needs 接线(#77)各自验过,但从未作为一个**完整自治 run** 一起跑过——"机制完整、整体 live 未证"在循环层面的同一风险。本次用真实工具/数据 live 证。

## live 测试发现的真实缺口(已修)

首跑(`agent run --apply --auto-run`,Noop seams)结果:agent **自主匹配**了 `local/survival_assoc`(确定性能力查询)并**起草了步骤**,但 apply 失败:
```
apply_failures: "flow validation failed: step ... is missing required param gene"
```
根因:`infer_replace_params` 只用 LLM `ParamInferer`/`CohortInferer`(此处 Noop)填参,**无确定性 gene 兜底**——尽管 `infer_gene_symbol`(确定性,只取全大写 gene 符号)已存在,只接在 synth 路径。

修复(镜像 AS19 的 `infer: cohort` 声明式模式):
- 新增 `ParamInferKind::Gene`;param 声明 `infer: gene` → `infer_replace_params` 用 `infer_gene_symbol(假设)` **确定性**填值。
- 填的值仍进 `inferred_param_names` → **grade-cap 仍生效**(自治 run 仍上不去 affirmed)。
- `examples/tools/local_survival_assoc.tool.yaml` 的 `gene` param 加 `infer: gene`。
- 顺带:交接提示文案 "LLM 推断的未确认参数" → "推断的未确认参数"(参数现可确定性推断,非必然 LLM)。

## 修复后 live 结果(agent 自行跑,我只分析)

`agent run --apply --auto-run --no-auto-synth --no-auto-forage --no-semantic-match`(纯确定性),输入:SPP1 假设 + 真实 TCGA-LIHC 切片。

```
Applied:
  lifecycle -> under_test
  flow auto_event_... auto-created            ← agent 自建流
  step step_survival_assoc ran and observed   ← agent 跑真分析
Outcome: handed_off
Decision: [stance_assessment] 分析步骤产出真实发现 ... 请判定它对假设的立场 ...
⚠ 该结果依赖推断的未确认参数:gene=SPP1(请人工确认)
```

agent **自主**产出的真实 marker_report(与 Plan A 参考逐位一致):
```
Gene: SPP1  score: 4.306  n: 365 (high 183 / low 182)
logrank_chi2: 16.4706  logrank_p: 4.94102e-05
direction: high-expression associated with worse overall survival
```
状态:flows 1 / steps 1 / runs 1 / 1 个 computed 工件;假设 → under_test;**未自治 affirm**,而是 raise `stance_assessment` 决策点交人判定立场。

## 结论

- **全自治闭环 live 证明**:假设 → 自主工具匹配 → 自主确定性填参 → 自主建流 → 自主跑真分析 → 真实证据 → **诚实交接**。
- **诚实不变量两路守住**:(1) 自治不自行判定 stance,raise StanceAssessment 交人;(2) 推断参数提示人工确认 + grade-cap 阻止自治 affirm。agent 做"工作",但把"判决"留给人。
- 这一轮也复用了"live test → 发现真 bug/缺口 → 修 → 重证"的模式(同 #90 容器修复)。

## 边界(诚实)

- 本次是**单步**自建流;多步链(step2 消费 step1 输出,触发调度器 fan-out)是自然后续——needs 接线 + scheduler 已就绪,但本次未触发多步。
- 确定性填参只覆盖 `gene`(全大写符号)与 `cohort`(AS19);其他 param 仍需 LLM `ParamInferer`。所以"完整自治"对一般 param 仍依赖 LLM;本次证的是**无 LLM 也能跑通**的确定性自治路径。
