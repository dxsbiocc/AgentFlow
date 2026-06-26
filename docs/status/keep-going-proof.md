# 验证记录:--keep-going(continue-on-error)执行语义

Date: 2026-06-26
Status: PASS — `run`/`agent run --keep-going`:某步失败时不再整条 run 停掉,而是继续跑其它可运行的步骤;失败步骤是**终态**(不重试),其下游被跳过。默认仍 fail-fast,逐字节不变。这是 #97 并行执行的 follow-up(部分失败语义)。

## 动机

#97 之前,顺序与并行路径都在**首个失败处停掉**整条 run。对有独立分支(fan-out)的流不友好:A 分支某步失败,B 分支即便完全独立也跟着不跑了——你只能一次修一个失败。`--keep-going`(make/ninja `-k` 语义)让一遍跑完所有能跑的步骤,一次看到所有失败 + 拿到所有独立结果。

## 语义(`RunConfig.keep_going`,默认 false)

- 失败步骤记入 `failed_ids`,**终态**:`ready_steps` 会把 failed 状态的步骤重新当 ready(为重试),keep-going 下据此过滤,不重试。
- 失败步骤不进 `completed` → 其**下游永不 ready → 被跳过**(只跳依赖失败的,无关步骤照跑)。
- 顺序路径:wave 内不再 `if failed break`(仅 fail-fast 时 break)。
- 并行路径 `run_ready_wave_parallel`:`batch_failed` 仅在 `!keep_going` 时停 wave。
- 终止:`ready` 为空即停(`failed_ids` 单调增、ready 单调减,保证收敛);仍保留 `!progressed` backstop。
- **默认 byte-identical**:`keep_going=false` 时 `ready` 不过滤、所有 break 照旧触发。

## 证据

- 单测 `keep_going_runs_independent_steps_after_a_failure`(顺序 + 并行 max_parallel=4):流含失败根 `bad`(先声明先跑)+ 独立成功分支 `good → good_tail` + 依赖失败的 `bad_tail`。
  - **fail-fast(默认)**:`failed=1, completed=0`,只跑了 `bad`(1 个 attempt)——`good` 没跑。
  - **keep-going**:`failed=1, completed=2`(`good`+`good_tail` 都跑),`bad_tail`(失败步下游)被跳过。
  - 并行路径同结果。
- CLI:`run` / `agent run` 都接受 `--keep-going`(clap + usage),`run <id> --keep-going` 解析正常。
- `argument.rs` 空 diff;core **362**(+1)+ cli + clippy + 两个 acceptance 脚本绿;无新依赖。

## 边界(诚实)

- 失败步骤**终态、不重试**(本次 run 内);要重试用 `retry`。
- 不做**在跑中取消**(in-flight cancellation):同一 wave 里已并发启动的兄弟步骤会各自跑完(它们相互独立,跑完无害),失败只影响**后续** wave / 下游。
