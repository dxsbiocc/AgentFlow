# 简报：MC.2a 运行期容器引擎选择(docker/podman,默认 docker)

Status: Assigned to Codex（worktree /tmp/af-mc2a，branch feat/mc2a-engine-selection，从 main 起）
RFC: docs/design/multi-engine-container-design.md §3.2 / §5(MC.2)。前置 MC.1(ContainerEngine + DockerEngine)已合并。本切片只做**引擎选择 plumbing + podman**;Singularity 留 MC.2b。

## 现状

- `run_command`(lib.rs:165)→ `store.run_flow(&flow_id)`(无引擎选项)。
- `run_flow`(mod.rs:628)→ `run_step`(:1027)→ 构造 `ExecContext`(:1075,字段 workdir/staged_inputs/output_dir/env_names)。
- `ContainerBackend`(backend.rs)用 `runtime.runner` + `DockerEngine`(MC.1)。引擎写死 docker。

## 目标(核心 Nextflow 价值:同一工具,run 期选引擎)

加一个**运行期引擎选择**:`run`/`agent run` 可选 `--container-engine docker|podman` + `--container-runner <path>`,默认 docker(行为不变)。该选择经 RunConfig → ExecContext 流到 ContainerBackend。podman 复用 DockerEngine(CLI 兼容)。**引擎不进缓存键**(同 image 任何引擎同结果)。

## 实现要求

1. **RunConfig**(mod.rs 或合适处):
   ```
   pub enum ContainerEngineKind { Docker, Podman }   // Singularity 留 MC.2b
   pub struct ContainerEngineSelection { pub kind: ContainerEngineKind, pub runner: Option<PathBuf> }
   #[derive(Default)] pub struct RunConfig { pub container_engine: Option<ContainerEngineSelection> }
   ```
2. **run_flow_with**:新增 `pub fn run_flow_with(&self, flow_id, config: &RunConfig)`;`run_flow(flow_id)` = `run_flow_with(flow_id, &RunConfig::default())`(**现有签名/行为不变**,所有现有调用方/测试不动)。run_step / run_step_ref 同理透传(run_step_ref 用 default 即可)。
3. **ExecContext 加** `container_engine: Option<&'a ContainerEngineSelection>`;run_step 从 RunConfig 填。其他后端忽略。
4. **ContainerBackend.prepare_command**:
   - 解析引擎:`ctx.container_engine` 有 → 用其 kind;无 → 默认 Docker(MC.1 行为)。
   - 解析 runner:override 的 `runner` 优先,否则 `runtime.runner`(保持 MC.1:tool 仍可声明 runner 作默认)。两者皆无 → 保持现有 "container runtime must declare absolute runner path" 错误。
   - Docker 与 Podman **都用 DockerEngine**(argv 一致,只是 runner 不同)。委托 `DockerEngine.build(runner, image, &runtime.command, ctx)`。
5. **CLI**:`RunArgs`(及 agent run 的 args)加 `--container-engine <docker|podman>` + `--container-runner <path>`(clap,Vec/Option 风格与现有一致);`run_command`/agent run 据此构造 RunConfig 传 `run_flow_with`。非法 engine 值报错。
6. **缓存键不变**:`runtime_config_json` **不加**引擎字段(引擎只改在哪跑)。现有精确字节缓存测试不改即过。

## 不变量(硬约束)

- `git diff crates/agentflow-core/src/argument.rs` 为空。
- **默认路径行为等价**:不传 `--container-engine` 时,一切与 MC.1 逐字节相同;现有所有测试断言不改即过。
- **引擎不进缓存键**:加一个测试断言——同 image 同 step,docker vs podman 选择下 `runtime_config_json`/缓存键**完全相同**(引擎不改结果身份)。
- podman 选择 → argv = DockerEngine 形状但 executable = podman runner。
- 仅改 `crates/agentflow-core`(mod.rs/backend.rs)+ `crates/agentflow-cli`(cli_args.rs/lib.rs);无新依赖;不加 Singularity。

## 测试(离线,低负荷)

- 现有测试原样通过(默认 docker 路径不变)。
- 新增:RunConfig 选 podman → ContainerBackend argv executable=podman runner、args 与 docker 形状一致;引擎不进缓存键(docker vs podman 同 image → 同 runtime_config_json);CLI flag 解析(docker/podman/非法)。
- 跑:`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test -p agentflow-core` + `cargo test -p agentflow-cli`(相关)、`bash scripts/acceptance-v1.sh`。**不要** `cargo test --workspace`。

不要 commit。报告:RunConfig/run_flow_with/ExecContext 改动、ContainerBackend 引擎解析、CLI flag、确认默认路径等价 + 引擎不进缓存键 + argument.rs 未动 + acceptance 绿。
