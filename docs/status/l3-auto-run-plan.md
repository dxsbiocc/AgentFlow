# L3 实现简报：循环自动运行分析 + 证据回灌（闭合自动链）

Status: Implemented + verified (2026-06-03)
Date: 2026-06-03
Owner(orchestrator): Claude · Executor: Codex（Rust + 本地 stub 工具离线测）· 编排者 live 验证
Spec source: L1/L2 后续 —— 闭合「假设→匹配工具→推参→apply→运行→observed 证据」自动链
Depends on: H7b-2（apply 路径）、L2（推参）、S2（grade 封顶）、runtime（run_step_ref/auto-observe）—— 均已合并 main

## 验收记录（Claude 独立复验 2026-06-03）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **191**（基线 188，+3）/ cli **61**（+2）/ schemas 3 全绿。
- ✅ 改动局限 `agent.rs`（auto_run + StepRun + auto_run_applied_step）+ `agent run` CLI；`argument.rs`/`handoff.rs`/`runtime/mod.rs`/`observer.rs` diff 全空（judge/cap/handoff 未改）；Cargo 零变更；无新表/event_type/依赖。
- ✅ auto_run 默认 false → 不传 `--auto-run` 时 --apply 行为零变化；现有测试不改且通过。
- ✅ 回灌 stance=`Neutral`，grade Observed 经 S2 对 exploratory 封顶为 `Inferred`；run 失败记入 `apply_failures` 不中断。
- ✅ **全自动链 live 验证**（本地 exploratory 工具 + 真 claude）：`agent run --apply --flow wf --infer-params --auto-run` → `applied=[graph_patch_applied(step_survival_marker), step_run(→observation)]`，证据回灌 `[inferred/neutral] auto-run`。即循环自主：假设→匹配工具→推出 gene=THRSP→apply→运行→observed 证据回灌，**无需人 draft/run**，且 Neutral+封顶→判决不自动 affirmed（防自欺在线）。

> 验收插曲（再记验证纪律）：两次 live 全链失败均为**编排者 smoke setup 错误**（flow 未注册 / tool command 内联格式），非 L3 缺陷——且 L3 的 `apply_failures` 容错正确捕获了「flow not found」未崩。setup 易错、机制可靠、**live 验证不可省**。

结论：合并就绪。自动研究链闭合——循环能自主跑完一轮真实分析并捕获证据；结论仍守在确定性判决 + 防自欺 + 人类 gate 之后（stance 自动判定为后续）。

## 目标

在循环 apply 步骤后，**自动运行该步骤** → 自动 observe（现有）→ **把观察回灌为证据**链到该假设。补上自动链最后一段。**opt-in `--auto-run`（默认关）**，需配 `--apply --flow`。

## 编排者裁决（约束）

1. **回灌 stance = `Neutral`，不自动判支持/反对**：分析跑了、结果记了，但「支持/反对」是解读，留人/后续 LLM stance 判定。Neutral 证据不移动判决——**循环自主跑分析、捕获证据，但不自动下结论**（一次 exploratory 分析 ≠ 定论，符合防自欺；且 tcga 等 exploratory 工具的证据本就被 S2 封顶）。
2. **opt-in 默认关**：`ApplyConfig` 加 `auto_run: bool`（默认 false）。不传 `--auto-run` 时**现有 --apply 行为逐字节不变**；现有 188 core / 59 cli 测试不改且通过（首要回归）。
3. **判决/封顶逻辑不动**：本步只在 apply 后新增「运行→回灌」；judge/cap/handoff 一概不改。
4. 不新增依赖/表；event_type 沿用现有（run/observe/argument.evidence_linked）。
5. 安全：auto-run 只在 `config.apply && config.auto_run && flow 提供` 时发生；运行有副作用（计算/网络）属预期，受 --apply 信封 + checkpoint 可回退保护。

## 交付物

### core `agent.rs`（apply 路径，仅 auto_run 分支）
- `ApplyConfig` 加 `auto_run: bool`（Default=false，补所有构造处）。
- apply 路径 `apply_graph_patch` 得 `applied_steps` 后，若 `config.auto_run`：
  - 对每个 applied `step_id`：`run_step_ref(step_id)`（失败 → 记入既有 `apply_failures` 并继续，不中断）。
  - 运行后查该 step 产生的观察（按 `source_step_id == step_id` 过滤 `list_observations()`，取最新）。
  - 若有观察：`link_evidence(EvidenceLinkRequest{ hypothesis_id = proposal.decision.candidate.hypothesis_id, observation_id = Some(obs.id), grade = Observed, stance = Neutral, note = "auto-run" })`（S2 会把 exploratory 来源封顶为 Inferred）。
  - 记 `AppliedAction`（新增变体如 `StepRun { step_id, observation_id: Option<String> }`，additive；`to_json` 补）。

### CLI `agent run`
- 加 `--auto-run`（bool，默认 false）→ `ApplyConfig.auto_run`。usage 追加。需配 `--apply`（无 apply 时 auto_run 无效/忽略，或文档说明仅 apply 下生效）。

## 验收标准（Claude 审核逐条核对）

- [ ] **回归**：不传 `--auto-run` 时 --apply 行为零变化；现有 188 core / 59 cli 测试不改且通过（首要）。
- [ ] `clippy -D warnings` 无警告；`cargo test` 净增、全绿；无新依赖/表/event_type。
- [ ] **离线行为测试**（用一个本地、无输入、无网络的 exploratory 工具，命令直接写出固定 marker_report；不依赖 tcga/网络）：注册该工具 + 建匹配假设 → `agent run --apply --flow <id> --auto-run` → applied step 被运行 → 其观察被回灌为 **Neutral** 证据链到该假设（`evidence_for` 可见）；grade 因 exploratory 被封顶为 inferred。
- [ ] 不传 `--auto-run` → apply 后不运行、不回灌（回归）。
- [ ] run 失败 → 记 apply_failures 并继续（不中断）。
- [ ] grep 确认 judge/cap/handoff 逻辑未改；auto-run 只在 apply+auto_run 分支。

## 不在本里程碑（明确排除，诚实声明）

- 自动判证据 stance（支持/反对，需 LLM stance）→ 后续；本步一律 Neutral。
- 因此判决不会因 auto-run 自动变 Affirmed（正确：需 stance + 足够强证据 + 人类 gate）。
- 真沙箱、并行运行、运行预算控制。
