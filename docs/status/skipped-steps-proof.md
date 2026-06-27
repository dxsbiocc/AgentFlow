# 验证记录:run 报告 skipped 步骤数

Date: 2026-06-27
Status: PASS — `run`/flow 执行的摘要新增 **Skipped steps**:从未执行的步骤(因依赖失败、或 fail-fast 提前停止)。与 `--keep-going`(#108)配套,让"哪些没跑"一目了然。

## 动机

`--keep-going` 让独立步骤在某步失败后继续跑;失败步骤的下游会被**跳过**(从不执行)。此前摘要只报 completed + failed,跳过的步骤不可见——用户看不出"3 完成 / 1 失败 / 2 跳过"里的跳过部分。fail-fast 模式下首个失败后剩余步骤也都被跳过,同样不可见。

## 改动

- `FlowRunSummary` 加 `pub skipped_steps: usize`。
- `run_flow_with`:循环结束后 `inspect_flow` 一次,统计仍为 `draft`/`ready`(从未跑)的步骤数 = skipped。不改 completed/failed/attempts,不改任何执行语义(纯报告)。
- `run_step_ref`(单步)`skipped_steps: 0`。
- CLI `run` 输出在 `Failed steps` 后加 `Skipped steps: {}`。
- `argument.rs` 不动。

## 证据

- 扩展单测 `keep_going_runs_independent_steps_after_a_failure`(4 步:`bad` 失败、`bad_tail` 依赖 bad、独立 `good`→`good_tail`):
  - fail-fast:只跑 `bad` → `skipped_steps == 3`。
  - keep-going:`good`+`good_tail` 跑、`bad` 失败 → `skipped_steps == 1`(仅 `bad_tail`)。
  - 并行(max_parallel=4)keep-going → `skipped_steps == 1`。
- core **363** + cli + clippy(workspace)+ acceptance 绿;`argument.rs` 空 diff;无新依赖。

## 协作说明

本次按用户要求尝试用 codex(`codex:codex-rescue`)实现:codex 以异步/后台方式启动后**未触达工作树**(launch 后 `git status` 干净、`FlowRunSummary` 无新字段)——与既往观察一致(后台 codex 拿不到交互式 Bash 授权)。遂由我在独立 worktree 直接实现并跑全门禁。详见 [[fable5-subagent-unavailable]] 记录的 codex 前台/后台差异。

## 边界

- skipped 只计"从未跑"(draft/ready);`running` 状态(崩溃残留)不计入 skipped(它是 stranded,另有处理)。
- 单步 `run --step` 路径 skipped 恒为 0(只跑一个步骤)。
