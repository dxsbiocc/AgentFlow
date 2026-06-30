# 验证记录:module 答案的按假设 param 推导(slice 4b-4c)

Date: 2026-06-30
Status: PASS — 当 agent 用 module 回答假设时(4b-3b 路径),module 答案 step 上**未设的、可推导(`infer:` 提示)的 param** 会从假设里推导填入(如按假设填 gene),并把推导出的参数名记入 `inferred_param_names` —— **诚实联锁**:推导值让判决保持 grade-cap,不能自治确认。补齐了 4b-3b 文档化的限制(module 答案此前只能用 module 固定 param)。`argument.rs` 字节不变。

## 动机

4b-3b 的限制:module 答案 step 的 param 全是 module YAML 里写死的,所以一个回答"基因 X 是否关联生存"的 module 无法按假设取 gene(否则每个基因一个 module,不实用)。4b-4c 让 module 答案 step 能按假设推导可推导的 param。

## 改动(`module_answer_proposal`,agent.rs)

- 签名增 `inferer` / `cohort_inferer`,返回 `Option<(EnrichedProposal, Vec<String>)>`(原先返回空 param 名)。
- 取出 observed 答案 step 后:查该 tool 的 param specs,对**有 `infer` 提示且 module 未设**的 param,补 `REPLACE_{name}` 占位;再调既有 `infer_replace_params` 从假设填入;返回推导出的参数名。
- 两处调用点(`enrich_branch_proposal`)线程化 inferer 并**返回推导名**(不再是 `Vec::new()`)。

**关键不变量(诚实)**:推导名必须上报,apply 路径据此记录"该参数是推断的"→ 判决 grade-cap。module 设了的 param(如 `cohort: FIXED_COHORT`)**不覆盖**;无 `infer` 提示的缺失必需 param 仍**保持缺失**(不静默填),照旧暴露为未解析。

## 可达性(关键洞察)

module 答案路径仅在 tool 路径无法成立时被采用。若唯一未解析的是可推导的 param,tool 路径会自己推导成功 → 不走 module。所以 4b-4c 真正激活的场景是:tool 路径被**另一个非可推导阻塞**挡住(如一个无 `infer` 的必需 param 或无法 chain 的输入)+ 答案 step 还有一个被 module 省略的**可推导** param。测试正是建模此场景。

## 证据

- **端到端 live 测试 `agent_infers_module_answer_param_from_hypothesis`**:`bio/report` 两个必需 param —— `marker`(`infer: gene`)+ `cohort`(无 infer);module 固定 `cohort: FIXED_COHORT`、**省略 `marker`**。tool 路径因 `cohort` 不可推导而回退 module;module 路径里 4b-4c 推导 `marker=TP53`(取自假设 "TP53 shows a survival association...")。断言:实例前缀步骤 `__bio_assoc_report__report ran and observed`(确为 module 路径)、observation 含 `TP53`(推导值入了答案)、无 "missing required"。
- 既有固定-marker 测试 `agent_answers_hypothesis_with_registered_module` 仍绿(module 设了 marker → 不补 REPLACE → 行为不变)。
- core **398** + cli + clippy(workspace)+ acceptance 绿;`argument.rs` 空 diff;无新依赖。

## 边界 / 4b-4 余下

- 仅推导有 `infer:` 提示的 param(gene/cohort);无提示的缺失必需 param 仍需 module 设值或报错。
- 4b-4 余下:(a) 链入 module 未满足输入(放宽 High-fit);(b) 嵌套 module;(d) module 成熟度/打分。
