# 简报：资源耗尽上限(#57)+ runtime 合成工具出网 guard(#59)

Status: Assigned to Codex（worktree /tmp/af-h5759，branch fix/hardening-dos-runtime-guard，从 main 起）
来源：发布前安全审计低 severity follow-up。两个 issue 都集中在 runtime/synth 执行路径(`runtime/mod.rs` + `synth_commands.rs` + `yaml.rs`)，合并一个任务做以避免改同一文件冲突。

## issue #57(MEDIUM,robustness):资源耗尽上限

恶意导入工件 / 合成脚本可耗尽内存：
- `crates/agentflow-core/src/runtime/mod.rs:~2419, ~2507`：`read_to_string` 读计算/导入工件(可多 GB)。
- `crates/agentflow-cli/src/synth_commands.rs:~2307`：validation 用 `wait_with_output`，恶意脚本可在超时前刷海量 stdout/stderr。
- `crates/agentflow-core/src/storage/yaml.rs:~9`：YAML 整篇读入解析，无大小/深度上限。

修复要求(最小、保守、可配置常量)：
- 为读取工件/结果/YAML 内容设**最大字节上限**(用具名 const，给合理默认，如工件/结果 ~64MiB、YAML ~4MiB；按现有 demo 数据规模留足余量，**不得**让现有 `examples/` 工件/测试超限失败)。超限返回明确错误，不 panic。
- validation/run 捕获的 stdout/stderr **截断**到上限(如 ~1MiB)，并在日志/错误里标注被截断。
- 文本/CSV 校验尽量用 `BufRead` 流式，避免一次性读全；对行数/列数给保守上限(若改动面过大，至少给字节上限 + 截断，核心是阻止无界读)。
- 所有上限用 const 定义、注释说明，便于将来调。

不要为了上限破坏现有合法路径：先确认 `examples/data/*` 与 acceptance demo 工件远小于上限。

## issue #59(LOW):runtime 执行的合成工具缺出网 guard

`PYTHON_EGRESS_GUARD_SITECUSTOMIZE`(synth_commands.rs)目前**只在 synth-time validation**(`python_command`，约 `:2382-2420`)注入；工具注册后经 `run_step`(`runtime/mod.rs:~956-971`)运行时**没有** guard/seatbelt。即"validation 有护栏、runtime 裸奔"的不对称。

修复要求(把合作层 guard 延伸到 runtime,保持诚实边界)：
- 对 **`namespace == "synth"`** 的工具(自动合成工具),在 `run_step` 构造运行命令时,把同一个 egress-guard sitecustomize 写入一个 guard 目录并 **PYTHONPATH 前置**(复用 validation 用的同一份 `PYTHON_EGRESS_GUARD_SITECUSTOMIZE` 常量与注入方式,不要复制第二份常量)。非 synth(用户声明的本地工具)保持现状,不强加 guard。
- guard 目录随运行清理思路同 isolated workdir;不改变工具脚本 argv/`__file__`。
- **诚实边界**:这是合作层 defense-in-depth(早失败/可读错误),**不是**反篡改沙箱;真封堵仍是部署级(issue #36 / `docs/ops/egress-containment.md`)。在代码注释和(若合适)`docs/CAPABILITIES.md` §6.5 / SECURITY.md 里点明 guard 现在 validation + runtime 都覆盖,但仍是合作层。

## 不变量(硬约束)

- `git diff crates/agentflow-core/src/argument.rs` 为空;不碰判决逻辑/DecisionKind。
- 不引入新依赖;guard 复用现有常量(不得新增第二份 sitecustomize)。
- 合成工具的核心/单次工件边界不变;不把单次任务脚本写进仓库源码。
- 现有 `examples/` 工件、acceptance、现有测试全绿(上限不得误伤)。

## 测试(离线,低负荷)

- #57:单测断言超大输入(合成,内存里构造略超上限的内容,**不写多 GB 文件**——用接近上限的小阈值测试或注入可调上限)被拒/截断并给明确错误;正常大小通过。stdout 截断有测试。
- #59:单测断言 synth-namespace 工具的 run 命令把 guard 目录前置进 PYTHONPATH(可断言命令构造/env);非 synth 工具不注入。复用现有 guard 常量(断言同一常量被用于 runtime)。
- `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test -p agentflow-core` + `cargo test -p agentflow-cli`(相关用例)、`bash scripts/acceptance-v1.sh`。**不要** `cargo test --workspace`(控制本机负荷)。

不要 commit(编排者来提交)。报告:改了哪些文件/行、新增测试、确认 argument.rs 未动、acceptance 绿、现有工件未误伤。
