# 简报：把 cohort 推断接进核心 run 循环的 param-filling（AS19，通用 seam，Noop 默认）

Status: Assigned to Codex（worktree /tmp/af-as19，branch feat/as19-cohort-param，从 main e96091c 起）
Spec source: AS18 PR(#52) 显式 deferred 项之一 —— "cohort inference into core param-filling"。

## 背景（当前事实，勿改其外）

- 核心 run 循环已有一个**通用 seam** `ParamInferer`（`crates/agentflow-core/src/agent.rs:93`，`NoopParamInferer` 默认；core 无 LLM/网络）。它在 param-filling 里按 param 名补值：`inferer.infer(hypothesis_statement, param_name)`（agent.rs:2007 附近，`infer_replace_params`）。任何被 seam 填的 param 会进 `inferred_param_names`，从而触发 grade-cap（`argument.rs:687` 把 observed→inferred）。这是**诚实属性**，必须保留。
- 另一个 seam `CohortInferer` **目前只在 CLI** 存在（`crates/agentflow-cli/src/agent_ops_commands.rs:695`），仅供 AS17 泛化验证门用；它的 grounded 实现 `LlmCohortInferer` 把 cohort 落在 cBioPortal `/api/studies` 且偏好 `pan_can_atlas`（AS17.1/17.2）。**核心 run 循环目前无法用 cohort 推断填 study/cohort 参数** —— 这正是要补的缺口。

## 目标

让自治 run 循环能用一个 **cohort 推断 seam** 填 cohort/study 类参数（autonomy proposes），且该参数被标记为 inferred（保持 grade-cap）。grounded 实现仍留在 CLI（网络），核心只加 Noop-default seam。

## 编排者裁决（实现，最小且通用）

### 1. 把 `CohortInferer` 提升为**核心 seam**（agent.rs）

- 在 `crates/agentflow-core/src/agent.rs` 新增 `pub trait CohortInferer { fn infer_cohort(&self, hypothesis_statement: &str) -> Option<String>; }` + `pub struct NoopCohortInferer;`（impl 返回 None），与 `ParamInferer`/`OutputGroundingScorer` 同款。
- CLI 删除其本地 `trait CohortInferer`/`NoopCohortInferer`（agent_ops_commands.rs:695-705），改 `use` 核心的；`LlmCohortInferer` 改 impl 核心 trait（方法名若不同则同步改）。**保持 CLI 行为不变**（grounding 不动）。

### 2. 核心 run 循环消费 cohort seam（agent.rs）

- 加一个接收 `&dyn CohortInferer` 的 run 变体（仿照现有 `run_cycle_with` → `run_cycle_with_*` 的下沉链；不要给每个老入口加参数，给一个新的 grounded 变体，老入口用 `&NoopCohortInferer` 委托）。
- 在 param-filling 里：当某个 param **仍未被 `ParamInferer` 填上**、且该 param 被声明为 **cohort 类**时，回退用 `cohort_inferer.infer_cohort(hypothesis)`；填上则把该 param 名加入 `inferred_param_names`（**必须**，触发 grade-cap）。
- **如何识别"cohort 类"param —— 用声明式信号，不要在引擎里硬编码 param 名字符串。** 优先复用 tool param spec：在 `ToolParamSpec` 加一个**可选** `infer: Option<ParamInferKind>`（enum `{ Cohort }`，serde 解析 yaml 里 `params.<x>.infer: cohort`）。引擎只看这个声明位，不认识 "study"/"cohort" 这些英文词 → 保持通用、无领域常量。未声明 `infer: cohort` 的 param 行为完全不变。

### 3. CLI 接线（agent_ops_commands.rs）

- 在实际 run（约 213-294 行的 run_cycle_with_* 分发）里，把已有的 grounded `LlmCohortInferer` 作为 cohort seam 传进新的 grounded run 变体（仅当 `options.semantic_match`/相应开关开启时，否则 `NoopCohortInferer`，与 gene_inferer 的 `infer_params` gating 同构）。不改 CLI flags 语义。

## 不变量（硬约束，违反则不收）

- `git diff crates/agentflow-core/src/argument.rs` **为空**；判决引擎 0 LLM/0 网络不变。
- 核心新增 seam **Noop 默认**；core 仍无 reqwest/http/llm（`grep -nE 'reqwest|http|llm|Llm|LLM' crates/agentflow-core/src/agent.rs` 不得新增网络调用）。
- 引擎**不得**出现硬编码的 cohort/study/疾病/基因英文词或 study id；cohort 识别只走声明式 `infer: cohort`。
- cohort-seam 填的 param **必入** `inferred_param_names`（写一个单测断言：cohort 填充后该 run 的证据被 cap 到 inferred、上不去 affirmed —— 复用 success_path_affirm.rs 的风格，纯合成、无网络）。
- 无新依赖。`ToolParamSpec.infer` 是**可选**字段，旧 yaml/旧注册向后兼容（缺省=None）。

## 测试（离线）

- core 单测：`NoopCohortInferer` 返回 None → 行为同今天；一个 stub CohortInferer 返回固定值 → 声明 `infer: cohort` 的 param 被填且进 inferred_param_names；未声明的 param 不受影响。
- 集成（合成，仿 success_path_affirm.rs）：verified 工具 + 一个 `infer: cohort` param 被 seam 填 → 证据被 cap 到 inferred → 非 affirmed（诚实门生效）。无 SPP1/疾病/真实数据/LLM/网络。
- `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`bash scripts/acceptance-v1.sh` 全绿。

## 不在本里程碑

- 不做 cohort 的 grounded 推断质量改进（AS17.1/17.2 已有，且留在 CLI）。
- 不自动注册/取代工具（那是 AS20 的事，另一个 worktree）。
- 不改 argument.rs / DecisionKind / 既有工件。
