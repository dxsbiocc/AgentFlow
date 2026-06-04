# ③ 实现简报：clap 重构 CLI 参数解析

Status: Assigned to Codex
Owner: Claude(编排) · Codex(执行)
Depends on: serde 迁移 ①② 全完成（已合并 main）

## 目标
把 agentflow-cli 的手写参数解析/分发（lib.rs 5427 行，61 顶层命令，52 handler）换成 **clap (derive)**——稳定、健壮、自动 help。**handler 业务逻辑与所有 `--json` 输出保持不变（byte-identical）**；改变的只允许是 `--help` 文本与参数错误信息格式。

## 验收策略（与 JSON 批次不同！）
- **NOT byte-identical for help/errors**：clap 的 `--help`、`missing required argument`/`unexpected argument` 等错误措辞会变，**接受**。
- **`--json` 输出必须 byte-identical**：由 handler 业务逻辑产生，重构后逐字节不变（编排者用 /tmp/clap_json_baseline.txt 对照：status/tools list/hypothesis list/evidence list 等）。
- **acceptance-v1.sh 必须通过**；**每个命令组都要能跑**（Codex 自测全命令冒烟后才算完成）。
- CLI 现有测试中断言「手写错误文案/unknown command 文案」的，更新为 clap 行为或改测行为/退出码；断言 `--json`/业务输出的保持。

## 约束
1) **完整镜像**现有全部命令/子命令/flag/positional/默认值/别名（如 `--path` 默认 cwd、`--json`、各子命令 register/list/inspect/... 等）。读 lib.rs 的 usage() + dispatch 取全集，不漏不改语义。
2) clap derive 命令树（`#[derive(Parser/Subcommand/Args)]`）；解析后调用 handler 业务逻辑（handler 可改为接收 clap 解析后的结构/字段，但**业务逻辑与输出不变**）。
3) 错误经 clap 上报，进程退出码与现有「错误退出非 0」一致；`agentflow` 无参/`--help`/`help` 给 usage。
4) 加 `clap = { version = "4", features = ["derive"] }` 到 agentflow-cli/Cargo.toml。不动 agentflow-core。
5) **绝不改 `--json` 输出**；绝不改任何命令的实际行为/副作用。

## 验收
- [ ] clippy -D warnings 干净；cargo test 全绿（更新后的 cli 测试）；acceptance-v1.sh 通过。
- [ ] **`--json` 输出 byte-identical**（编排者对照基线）。
- [ ] 全命令组冒烟通过：init/status/doctor/tools(register/list/inspect/match/draft-step)/env/import/artifacts/flow/run/run-step/report(+research)/cache/retry/observe/observations/research/hypothesis/evidence/verdict/branch/decision/forage/trace/agent/synth/patch/compare/runs/logs。
- [ ] 现有命令/flag 无遗漏（与 usage() 对照）。
- [ ] 只动 agentflow-cli；agentflow-core 未改。

## 不在本里程碑
新增命令/flag；改变任何命令行为或 --json 形状。

## 编排者验收：phase 1 打回（2026-06-04）
Codex 首版**功能通过但架构是半吊子**，打回重做：
- 现状：clap 解析→类型结构体→`into_args()`(69个)重建 argv→旧手写解析器(58 handler, 268 处 next_arg/--path)再解析。**双重解析**。
- 问题：手写解析**一行没删**，反而上面叠了 clap + 69 函数回灌胶水。**三套解析面，稳定性更差**，与「不手搓、增稳定」目标相反。
- 保留：cli_args.rs 的 clap 命令树（已验证 --json byte-identical，是对的）。
- phase 2 要求：删除全部 `into_args()`；每个 `*_command` handler 改为**按值接收其 clap Args 结构体**、直接用类型字段；**删光 handler 内手写解析**（next_arg/expect_value/各 parse flag）。业务逻辑 + --json 输出不变。

## 验收记录（Claude 独立复验 2026-06-04，phase 2）
- ✅ **手写解析真清零**：`into_args`=0、`next_arg`=0、`expect_value`=0（grep）；handler 直接吃 clap typed structs。
- ✅ clippy -D warnings 干净；cargo test 全绿（cli 57 + 结构回归 2 + core 234 + schemas 3）；acceptance 通过。
- ✅ **--json 逐字节一致 vs 重构前基线**（status/tools list/hypothesis list/evidence list，唯一差异是 temp 路径名）。
- ✅ **命令面零遗漏**：main 全部 57 个二级子命令 clap 均接受（逐一对照）；flow/branch/verdict/trace 子命令集与 main 精确吻合。
- ✅ core/schemas 未改；仅 agentflow-cli + 新增 clap 依赖。
- ✅ 删除的 4 个 `agent_run_parses_*_flags` 测试针对已删的手写 `parse_agent_run_options`；flag(auto-run/auto-forage/propose-synth/infer-params) 仍在 clap 树、--help 可见，行为保留。
结论：合并就绪。clap 接管 CLI 解析，手写参数解析彻底删除，单一健壮解析面。serde 迁移①②③ 全部完成。
