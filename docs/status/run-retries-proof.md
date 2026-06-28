# 验证记录:`run --retries N`(瞬时失败自动重试)

Date: 2026-06-28
Status: PASS — `run --retries N` 让失败的步骤在判定为"终态失败"前最多重跑 N 次(共 N+1 次尝试)。默认 `0` 行为不变(失败即终态)。适用于 flaky 工具(网络、外部脚本)。串行与并行路径统一生效。

## 动机

研究运行时大量 shell out 到外部工具/脚本(网络请求、第三方 CLI),这些会**瞬时**失败。此前任一步骤失败即终态:fail-fast 直接停、keep-going 跳过下游。一个可调的自动重试预算能把瞬时抖动和真正的失败区分开。

## 设计(循环级,统一覆盖串/并行)

- `RunConfig.retries: usize`(derive Default → 0)。
- `run_flow_with` 循环已知 `ready_steps` **会重新纳入 `failed` 状态的步骤**(原本用于手动 retry)。新增 `retry_counts: BTreeMap<step_id, usize>`:某步失败时 `record_step_failure` 递增计数——**预算内**(`tries <= retries`)返回 transient,不计 failed、不加 failed_ids、设 `retrying=true`,下一轮自然被重新 offer 重跑;**预算耗尽**才计为终态失败(failed_steps++、failed_ids、触发 fail-fast)。
- 无进展退出条件由 `!progressed` 改为 `!progressed && !retrying`,确保单个 flaky 步骤不会因"本轮无成功"被提前判死;计数单调递增 → 至多 N+1 次 → **有界,绝不死循环**。
- CLI:`run --retries <n>`(clap 校验 usize),usage 增项。`agent run` 自动运行路径本次不接(后续可加)。
- `argument.rs` 不动;纯执行可靠性,判决语义无关。

## 证据

- 单测 `retries_re_run_a_transient_step_failure`(计数文件驱动的 flaky 工具,第 k 次才成功):
  - `--retries 0`,第 2 次才成功 → 1 次尝试、completed=0 failed=1(失败即终态)。
  - `--retries 1`,第 2 次成功 → completed=1 failed=0 skipped=0、attempts=2(瞬时失败被吸收,运行完成)。
  - `--retries 1`,需第 5 次才成功 → 预算耗尽:completed=0 failed=1、attempts=2(**有界**)。
- CLI:`run --help` 含 `--retries <n>`;非数值被拒。
- core **364**(+1)+ cli + clippy(workspace)+ acceptance 绿;`argument.rs` 空 diff;无新依赖。

## 边界(诚实)

- 重试**立即**进行,无退避(backoff)——后续可加。
- 仅 `run` 命令接 `--retries`;`agent run` 自动运行未接。
- 重试会重跑工具(对确定性/可缓存的 producer 安全;有副作用的工具需自行幂等)。
