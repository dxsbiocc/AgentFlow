# 简报：P-C.1b 容器后端(--network none + 只挂 workdir + 镜像进缓存键,关 #36)

Status: Assigned to Codex（worktree /tmp/af-pc1b，branch feat/pc1b-container-backend，从 main 起）
RFC: docs/design/container-backend-design.md §3 / §5(P-C.1b)。前置 P-C.1a(ExecContext)已合并。

## 目标

新增 `runtime.backend: container`:在容器内跑工具命令,**只挂 per-step workdir**(硬 FS 隔离——看不到 artifact store/宿主)、**默认 `--network none`**(关 #36 的硬出网封堵)、镜像 digest 进缓存键。`prepare_command` 纯构造 docker argv → **离线断言 argv 即可测,不真跑 Docker**。

## 实现要求

### 1. ToolRuntimeSpec 加 image
- `ToolRuntimeSpec.image: Option<String>`(+ `RawToolRuntimeSpec` yaml 解析 `image:` + `into_runtime`)。
- 校验(tool_registry validate):`backend == "container"` 时**必须**声明 `image` 且 `runner`(容器运行时可执行路径,如 /usr/bin/docker 或 podman);非 container 后端不允许 image(或忽略,二选一,简报作者判断,但保持向后兼容)。

### 2. ContainerBackend(backend.rs)
`prepare_command(runtime, ctx)` 产出 PreparedRuntimeCommand:
- executable = `runtime.runner`(docker/podman 路径)。
- args = `["run","--rm","--network","none","-v", format!("{}:{}", ctx.workdir, ctx.workdir), "-w", ctx.workdir]`
  + 对 `ctx.env_names` 每个 `-e <NAME>`(转发宿主已设的 AGENTFLOW_* 进容器)
  + `image`(原样,若含 `@sha256:` 即 digest 钉死)
  + `runtime.command...`(工具命令)。
- 挂载点用**同路径**(`workdir:workdir`)→ workdir 内的 staged input/output 绝对路径在容器内仍有效。

### 3. ExecContext 加 env_names(P-C.1a 的 struct 追加字段)
- `ExecContext.env_names: &'a [String]` —— run_step 注入的 AGENTFLOW_* 变量名集合(inputs/params/outputs 各 port 名 + AGENTFLOW_WORKDIR/*_JSON 等),供容器 `-e` 转发。
- run_step 收集它实际 `.env(...)` 设置的 AGENTFLOW_ 前缀变量名(mod.rs ~1183-1194 及 per-port AGENTFLOW_INPUT_*/PARAM_*/OUTPUT_* 注入处)成一个 Vec,放进 ExecContext。
- 其他后端忽略 env_names(行为不变)。

### 4. 缓存键(runtime_config_json)
- 加 `container_image: Option<String>`(**Option + `#[serde(default, skip_serializing_if = "Option::is_none")]`**),仅 container 后端非空。
- **旧后端(local/conda/isolated)`runtime_config_json` 输出逐字节不变**(现有精确字节测试不改即过)——同 P1.2 `isolated_env_lock` 手法。
- tool_registry `stored_json`/`spec_hash` 若需含 image,同样 Option 跳过空,保证旧工具 spec_hash 不变。

### 5. 路由 + 文档
- `backend_for("container")` 返回 ContainerBackend。
- CAPABILITIES 执行隔离节:容器后端硬封堵(--network none + 只挂 workdir)已落地,关 #36 的运行时硬隔离(allowlist/真 Docker 验证留后续);README 能力表;issue #36 可在文档标注可关闭(Codex 不动 gh)。

## 不变量(硬约束)

- `git diff crates/agentflow-core/src/argument.rs` 为空。
- **容器只改"在哪跑"不改结果**:回归测试断言容器 argv 含 `--network none`、`-v <workdir>:<workdir>`、`-w <workdir>`、image、`-e` 转发,且**工具命令原样在末尾**(命令不变 → 同输入同镜像同结果)。
- 旧后端 prepare_command argv + runtime_config_json + spec_hash **逐字节不变**;现有测试不改即过。
- 不真跑 Docker(只断言 argv);离线、低负荷。
- 仅改 `crates/agentflow-core`(backend.rs/mod.rs/tool_registry.rs)+ 文档;无新依赖;`unsafe_code=forbid` 不破。

## 测试(离线)

- ContainerBackend.prepare_command:给定 image+runner+ctx → argv 含上述要素、工具命令在末尾、env_names 转成 `-e`。
- 缺 image/runner → 注册校验报错。
- local/conda/isolated 的 runtime_config_json/spec_hash 逐字节不变(新增 container 的 config 含 container_image)。
- 现有测试断言不改即过。
- 跑:`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test -p agentflow-core`、`bash scripts/acceptance-v1.sh`。**不要** `cargo test --workspace`。

不要 commit。报告:image 字段/校验、ContainerBackend argv、ExecContext.env_names 与 run_step 收集点、缓存键如何保证旧后端字节不变、argument.rs 未动、现有测试未改即过、acceptance 绿。
