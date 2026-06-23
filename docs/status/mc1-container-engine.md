# 简报：MC.1 ContainerEngine 抽象 + DockerEngine(行为等价重构)

Status: Assigned to Codex（worktree /tmp/af-mc1，branch feat/mc1-container-engine，从 main 起）
RFC: docs/design/multi-engine-container-design.md §3.3 / §5(MC.1)。为多引擎(Singularity/podman)铺路。

## 现状

`ContainerBackend`(`crates/agentflow-core/src/runtime/backend.rs:135`)的 `prepare_command` 直接构造 docker 风格 argv:
```
<runner> run --rm --network none -v <wd>:<wd> -w <wd> -e NAME... <image> <command...>
```

## 目标(纯重构,行为字节级等价)

把"docker argv 构造"抽到一个 `ContainerEngine` 抽象后的 `DockerEngine` 里;`ContainerBackend` 委托给它。**本切片只有 docker 一个引擎**(默认),argv 与现在**逐字节一致**。不引入引擎选择 config、不加 Singularity(那是 MC.2)。

## 实现要求

1. 在 backend.rs(或新 `runtime/container.rs` 子模块,`mod container;`)定义:
   ```
   pub(super) trait ContainerEngine {
       fn build(&self, runner: &str, image: &str, command: &[String], ctx: &ExecContext)
           -> PreparedRuntimeCommand;
   }
   pub(super) struct DockerEngine;   // = podman(CLI 兼容),本切片只此一个
   ```
   `DockerEngine.build` 产出与现 ContainerBackend **完全相同**的 argv:`run --rm --network none -v wd:wd -w wd -e NAME...(按 ctx.env_names) <image> <command...>`,executable = runner。
2. `ContainerBackend.prepare_command` 改为:校验 runner + image(保持现有错误文案),然后 `DockerEngine.build(runner, image, &runtime.command, ctx)`。逻辑搬走,行为不变。
3. 不改 `backend_for`、不改 ToolRuntimeSpec、不加 CLI/config。

## 不变量(硬约束)

- `git diff crates/agentflow-core/src/argument.rs` 为空。
- **行为等价**:现有 container argv 测试(断言 --network none / -v / -w / image / -e / 命令在末尾)**断言不改**即通过——DockerEngine 产出与重构前逐字节一致。
- 仅改 `crates/agentflow-core/src/runtime/`(backend.rs + 可选 container.rs);不碰 storage / CLI / argument.rs。无新依赖。
- 不加 Singularity / 引擎选择(MC.2)。

## 测试(离线,低负荷)

- 现有 container 测试原样通过(等价证明)。
- 可加一个小测:`DockerEngine.build(...)` 的 argv 与一个已知期望逐字节相等(锁住 docker argv 形状,为 MC.2 加 Singularity 时对照)。
- 跑:`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test -p agentflow-core`、`bash scripts/acceptance-v1.sh`。**不要** `cargo test --workspace`。

不要 commit。报告:ContainerEngine/DockerEngine 位置、ContainerBackend 委托改动、确认现有 container 测试未改即过 + argument.rs 未动 + acceptance 绿。
