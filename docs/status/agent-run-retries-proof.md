# 验证记录:`agent run --retries N`(自治运行的重试预算)

Date: 2026-06-28
Status: PASS — `agent run` 现接受全局 `--retries N`,并把它带入自治 auto-run 所用的 `RunConfig`(经 `with_run_config` 线程局部作用域),与既有的 `--max-parallel` / `--keep-going` 一致。补齐自治运行的可靠性旋钮。默认 `0` 行为不变。

## 动机

`run --retries N`(#118)给手动跑 flow 加了瞬时失败自动重试。自治路径 `agent run --auto-run` 此前只有 `--max-parallel` / `--keep-going`,缺 `--retries`——agent 自建并 auto-run 的 flow 里有 flaky 步骤时无法用同一旋钮重试。本次补上。

## 改动(纯线程化,镜像 `--keep-going`)

- `AgentArgs` 加 `retries: Vec<usize>`(`global = true`,可出现在 `agent run` 之后)。
- `agent_command`:解构 `retries`,`run_config.retries = last_value(retries).unwrap_or(0)`,经 `with_run_config(&run_config, …)` 注入作用域;重建的 `AgentArgs` 补 `retries: Vec::new()`。
- usage 增项;`argument.rs` 不动。

## 证据 / 测试取舍(诚实)

- **行为正确性已由 #118 核心测试覆盖**(`run_flow_with` 的 retry 直接测:off→失败、recovers、exhausted→有界)。
- **本次新增 clap 解析单测**(`cli_args.rs`):`run --retries 3` → `retries == [3]`;`agent run --apply --retries 2`(全局旗标)→ `agent.retries == [2]` 且子命令为 Run;`--retries abc` 被拒。干净证明旗标已接入两个命令的参数结构。
- **为何不加 agent-run 行为测试(诚实说明)**:`agent run` 单次调用内会做**多轮 apply 迭代**,且计数文件型 flaky 工具会**跨迭代**恢复——所以无论 `--retries` 取 0 还是 1,自治链最终都会跑完,行为上**无法隔离**该旗标的效果(实测 `--retries 0` 与 `--retries 1` 均使 consumer 最终运行;`--max-apply 1` 又会破坏链的搭建)。故 agent-run 层用解析测试 + 复用 #118 行为测试,而非写一个会产生误导性绿灯的行为测试。
- core **364** + cli(+3 解析单测)+ clippy(workspace)+ acceptance 绿;`argument.rs` 空 diff;无新依赖。

## 边界

- `--retries` 仅作用于自治 auto-run 实际执行 flow 的那一步(`run_flow_with`);agent 的多轮 apply 迭代本就有自身的重试效果。
- 重试立即、无退避(backoff)——后续可加(对 `run` 与 `agent run` 一处加即可)。
