# L1 实现简报：LLM 参数推断（让循环自主填工具参数）

Status: Implemented + verified (2026-06-03)
Date: 2026-06-03
Owner(orchestrator): Claude · Executor: Codex（Rust + stub 离线测）· 编排者 live 验证（claude）
Spec source: 用户选定 —— LLM 第一个安全入口；解锁「连 gene=THRSP 都要人填」缺口
Depends on: T1（draft_step_for）、S1（synth_commands 的 LLM 子进程原语）—— 均已合并 main

## 验收记录（Claude 独立复验 2026-06-03）

- ✅ `clippy -D warnings` 无警告；`cargo test` cli **57**（基线 54，+3）/ core **186（未改）** / schemas 3 全绿。
- ✅ 改动仅 CLI（`lib.rs` draft-step + `synth_commands.rs` 可见性 pub(crate)）；**core 零改动**；Cargo 零变更；无新表/event_type/依赖。
- ✅ 默认零回归：无 `--infer-params` → `gene: REPLACE_gene`（live）。
- ✅ **真 LLM live 验证**：假设「High THRSP expression is protective in hepatocellular carcinoma」+ `--infer-params --synthesizer "claude -p"` → 推断出 **`gene: THRSP`**。
- ✅ 空推断保留占位、`--infer-params` 缺 `--hypothesis` 报错（stub 测试）。
- ✅ 未碰 ArgumentEngine/判决/apply。

结论：合并就绪。LLM 第一个安全入口落地——从假设自主提取工具参数，解锁「循环自主跑对的分析」。判决仍确定性，LLM 只做语义提取（下游 tool 运行验证）。L2（接进 `agent run --apply` 自动填参并跑）为后续。

## 目标

给 `tools draft-step` 加 `--infer-params`：用 LLM 从**假设语句**推断工具的**参数值**，把 `REPLACE_<name>` 占位符替换成真实值（如 `gene: THRSP`）。这让循环能自主跑对的分析。**判决引擎 ArgumentEngine 绝不碰**——LLM 只做语义提取（喂给确定性机器），参数错了下游 tool 运行/验证会暴露。

## 编排者裁决（约束）

1. **全在 CLI**：改动 `lib.rs`（`tools_draft_step_command` + 选项解析 + usage）+ 复用 `synth_commands.rs` 的 `run_synthesizer`/`split_synthesizer_command`/`strip_markdown_fence`（按需提 `pub(crate)`）。**core 不改**（用现有 `draft_step_for` + `inspect_hypothesis`）。
2. **后端可配** `--synthesizer "<cmd>"`（默认 `claude -p`）；stub 离线测、claude live 验证。
3. **只推断占位参数**：仅替换值为 `REPLACE_<name>` 的必填参数；LLM 返回空/失败 → 保留占位符（不编造、不中断）。
4. 不新增依赖；不新增 event_type/表；现有 54 cli / 186 core 测试不改且通过。
5. **不碰判决/不自动 apply**：本步只产出带推断参数的 ProposedStep（人或后续循环再用）。

## 交付物

### `tools draft-step` 扩展
```
agentflow tools draft-step <tool-ref> [--input <type>:<id>]... \
   [--hypothesis <id>] [--infer-params] [--synthesizer "<cmd>"] [--json] [--path <p>]
```
- `--infer-params` 需配 `--hypothesis <id>`（否则 `InvalidArgument`）。
- 流程：`inspect_hypothesis(id)` 取 statement → `draft_step_for(tool_ref, inputs)` 得 step → 对每个值为 `REPLACE_<name>` 的 param：
  - prompt（固定模板）：`Research hypothesis: "<statement>". A bioinformatics analysis tool needs a value for the parameter "<param_name>". Reply with ONLY the value (e.g. a gene symbol like THRSP), no explanation, no quotes.`
  - `run_synthesizer(synthesizer, prompt)` → `strip_markdown_fence` → trim 取首行非空 → 非空则替换该 param 值；空/出错则保留占位符。
- 输出 ProposedStep（`--json` 同现有格式，params 已填）。
- 不传 `--infer-params` → 行为与现状完全一致（回归）。

## 验收标准（Claude 审核逐条核对）

- [ ] `clippy -D warnings` 无警告；`cargo test` 净增、全绿；现有 54 cli / 186 core 测试不改；**core 未改**。
- [ ] 无新依赖/表/event_type。
- [ ] 不传 `--infer-params` 时 `tools draft-step` 行为零变化（回归测试）。
- [ ] **离线 stub 测试**：①注册带 param 的工具 + 建假设 → `--infer-params --synthesizer <stub 输出 "THRSP">` → step 的该 param 值 = `THRSP`（非 `REPLACE_`）；②stub 输出空 → 保留 `REPLACE_<name>`；③`--infer-params` 缺 `--hypothesis` → 错误。
- [ ] grep 确认改动仅 lib.rs + synth_commands 可见性；core 未改；未碰 ArgumentEngine/判决/apply。

## 不在本里程碑（明确排除，诚实声明）

- 把参数推断接进 `agent run --apply` 让循环自动填参并跑（→ L2 后续，需把推断做成循环可调的 hook）。
- LLM 进 BranchSelector / stance 判定 / 判决引擎。
- 推断参数的正确性校验（本步靠下游 tool 运行 + 验证 + 确定性判决暴露错误）。
