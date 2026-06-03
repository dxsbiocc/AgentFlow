# L2 实现简报：参数推断接进自治循环（ParamInferer trait 缝）

Status: Implemented + verified (2026-06-03)
Date: 2026-06-03
Owner(orchestrator): Claude · Executor: Codex（Rust + stub 离线测）· 编排者 live 验证（claude）
Spec source: L1 后续 —— 让循环（而非手动命令）自主填工具参数
Depends on: L1（参数推断 + synth 子进程原语）、H7a/T2（run_cycle/enrich）、H2（BranchCandidate.statement）—— 均已合并 main

## 验收记录（Claude 独立复验 2026-06-03）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **188**（基线 186，+2）/ cli **59**（+2）/ schemas 3 全绿。
- ✅ `ParamInferer` trait + `NoopParamInferer` 在 core（dep-free）；`run_cycle_with(config, &dyn ParamInferer)` 新增，旧 `run_cycle_with_apply_config` 委托 Noop；现有调用点/测试零改动。core 零新依赖；Cargo 零变更；无新表/event_type。
- ✅ 判决/ArgumentEngine/apply 逻辑未改（仅 enrich 参数填充）。
- ✅ 默认零回归：无 `--infer-params` → 提议参数 `gene: REPLACE_gene`（live）。
- ✅ **循环 + 真 LLM live 验证**：THRSP 假设 → 循环自主匹配 `tcga/survival_assoc` → `--infer-params --synthesizer "claude -p"` → 自主推出 `gene: THRSP`。
- ✅ StubParamInferer 离线测（填参 / Noop 保留占位）。

结论：合并就绪。LLM 经 `ParamInferer` trait 缝接进循环——循环自主把假设变成带真实参数的可跑步骤。判决仍确定性。**剩余**：循环自动 RUN 已 apply 的步骤 + 自动 observe + link 回假设（→ 后续），即「从假设到结论全自动」的最后一段。

## 目标

把 L1 的参数推断接进 `run_cycle`：循环 enrich 提议时，用一个 **`ParamInferer` trait** 把 `drafted_step` 的 `REPLACE_<name>` 占位参数填成真值（如 `gene: THRSP`）。这正是用户要的「**LLM 接进循环引擎**」的正确形态——core 定义 trait 缝（dep-free），LLM 实现放 CLI（子进程）。**判决引擎 ArgumentEngine 仍不碰。**

## 编排者裁决（约束）

1. **trait 缝在 core，LLM 实现在 CLI**：core 只定义 `trait ParamInferer` + `NoopParamInferer`（dep-free 接口）；LLM 子进程实现放 `agent_ops_commands.rs`/`synth_commands.rs`。core 不引入任何依赖。
2. **最小 churn + 零回归**：新增 `run_cycle_with(config, inferer: &dyn ParamInferer)`；旧 `run_cycle_with_apply_config(config)` 委托 `run_cycle_with(config, &NoopParamInferer)`（现有全部调用点/测试**零改动**，行为完全不变——Noop 永远返回 None → 保留占位）。
3. **判决/apply 安全不变**：H7b-2 的 apply、强判决交接、S2 封顶等一概不动；本步只改 enrich 时的参数填充。
4. 不新增依赖/表/event_type；现有 186 core / 57 cli 测试不改且通过（首要回归）。

## 交付物

### 1. core `agent.rs`
```rust
pub trait ParamInferer {
    /// 从假设语句推断某参数值；None = 不填（保留占位）。
    fn infer(&self, hypothesis_statement: &str, param_name: &str) -> Option<String>;
}
pub struct NoopParamInferer;
impl ParamInferer for NoopParamInferer { fn infer(&self,_:&str,_:&str)->Option<String>{None} }
```
- `run_cycle_with(&self, config: ApplyConfig, inferer: &dyn ParamInferer) -> Result<CycleReport, StorageError>`：现 `run_cycle_with_apply_config` 的逻辑搬来，多接 `inferer`。
- `run_cycle_with_apply_config(config)` 改为 `self.run_cycle_with(config, &NoopParamInferer)`（保持签名与行为）。
- `enrich_branch_proposal` 多接 `inferer: &dyn ParamInferer`：`draft_step_for` 后，对每个值为 `REPLACE_<name>` 的 param，调 `inferer.infer(&candidate.statement, name)`；`Some(非空 trim)` 则替换，否则保留占位。

### 2. CLI `LlmParamInferer`
- 在 `agent_ops_commands.rs`（或 synth_commands）实现 `ParamInferer`：`infer` 用 `run_synthesizer(synthesizer, prompt)` + `strip_markdown_fence` + trim 首行；空/出错返回 None。prompt 同 L1 模板。
- `agent run` 加 `--infer-params [--synthesizer "<cmd>"]`（默认后端 `claude -p`）：传则 `run_cycle_with(config, &LlmParamInferer{..})`；否则走默认（`run_cycle_with_apply_config` = Noop）。

## 验收标准（Claude 审核逐条核对）

- [ ] **回归**：不传 `--infer-params` 时行为零变化；现有 186 core / 57 cli 测试不改且通过（首要）。
- [ ] `clippy -D warnings` 无警告；`cargo test` 净增、全绿。
- [ ] core 不引入依赖（仅 trait + Noop）；无新表/event_type。
- [ ] 行为测试（core，用 **StubParamInferer** 返回固定值，离线）：①有匹配工具的提议 → `drafted_step` 的占位 param 被 Stub 值替换；②`NoopParamInferer` → 保留 `REPLACE_<name>`（回归）。
- [ ] CLI 测试：`agent run --infer-params` 用 stub 后端 → 提议参数被填；不传 → 占位。
- [ ] grep 确认未碰 ArgumentEngine/判决；apply 路径逻辑未改（仅 enrich 参数填充变化）。

## 不在本里程碑（明确排除）

循环自动 RUN 已 apply 的步骤 + 自动 observe + 自动 link 回假设（→ 后续；当前 apply 只加步骤不执行）、LLM 进 BranchSelector/stance/判决、推断正确性校验（靠下游运行+判决暴露）。
