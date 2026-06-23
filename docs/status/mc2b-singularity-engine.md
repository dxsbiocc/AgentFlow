# 简报：MC.2b SingularityEngine(HPC,Apptainer)

Status: Assigned to Codex（worktree /tmp/af-mc2b，branch feat/mc2b-singularity，从 main 起）
RFC: docs/design/multi-engine-container-design.md §3.3(SingularityEngine)。前置 MC.1(ContainerEngine/DockerEngine)、MC.2a(引擎选择 docker/podman + RunConfig + ExecContext.container_engine)已合并。

## 现状

- `ContainerEngineKind { Docker, Podman }`(backend.rs);`ContainerBackend.prepare_command` 对 Docker|Podman 用 `DockerEngine.build`(`<runner> run --rm --network none -v wd:wd -w wd -e NAME... <image> <cmd>`)。
- run_step(mod.rs:1246-1256)spawn 时 `env_clear().env(PATH).env(AGENTFLOW_WORKDIR/*_JSON...).envs(step_env_vars)`;docker 用 argv 里的 `-e NAME` 把这些从宿主 env 转发进容器。

## 目标

加 `ContainerEngineKind::Singularity`(`apptainer` 作别名)+ `SingularityEngine`,让 `--container-engine singularity` 跑 HPC 容器。**引擎仍不进缓存键**(同 image,docker/singularity 同结果身份)。**argv 离线测试**(对照 MC.1 锁的 docker 形状),不真跑——CAPABILITIES 已声明容器后端 live 未证。

## 实现要求

### 1. 引擎枚举 + 解析
- `ContainerEngineKind` 加 `Singularity`。CLI `--container-engine` 接受 `singularity` 与 `apptainer`(都映射到 Singularity)。
- `ContainerBackend.prepare_command` match:Docker|Podman → DockerEngine;Singularity → SingularityEngine。

### 2. SingularityEngine.build(runner, image, command, ctx)
argv(executable = runner,即 singularity/apptainer 路径):
```
exec --containall --net --network none -B <wd>:<wd> --pwd <wd> <image> <command...>
```
- `--containall`:关掉 singularity 默认挂的 $HOME/$PWD/tmp,达到与 docker 等价的硬隔离。
- `--net --network none`:默认无网(singularity 用 net namespace + none CNI)。
- `-B <wd>:<wd>`:bind mount per-step workdir(同路径,staged I/O 绝对路径在容器内有效)。
- `--pwd <wd>`:容器内工作目录。
- **不发 env flag**(singularity 无 docker 式 `-e`);env 走 SINGULARITYENV_(见 §3)。
- image 原样(可 `docker://ref` 或 `.sif`);command 原样在末尾。

### 3. run_step:SINGULARITYENV_ 转发
- singularity 不读 docker 式 `-e`;它把宿主 `SINGULARITYENV_<NAME>` 自动转发为容器内 `<NAME>`。
- run_step spawn 前:**当解析出的引擎是 Singularity 时**(检查 `config.container_engine` 的 kind),对每个 AGENTFLOW_* 变量(AGENTFLOW_WORKDIR、*_JSON、step_env_vars 里的 per-port INPUT/PARAM/OUTPUT)额外 `.env(format!("SINGULARITYENV_{name}"), value)`。非 singularity 不加(no-op)。
- 这样容器内仍拿到 `AGENTFLOW_INPUT_*` 等(与 docker 行为对齐)。

### 4. 缓存键 / 不变量
- **引擎不进缓存键**:`runtime_config_json`/`RuntimeConfigJson` 不动(MC.2a 已保证)。加测试:同 image 同 step,docker vs singularity 选择 → `runtime_config_json` **逐字节相同**。
- `git diff crates/agentflow-core/src/argument.rs` 为空。
- 默认路径(docker)与 MC.2a **逐字节不变**;现有测试断言不改即过。

## 测试(离线,低负荷)

- `SingularityEngine.build` argv:含 `exec --containall --net --network none -B wd:wd --pwd wd <image>`,command 在末尾,**无 `-e`/`run`**(与 docker 形状不同)。
- CLI `--container-engine singularity`/`apptainer` 解析为 Singularity;非法值报错。
- SINGULARITYENV_ 转发:singularity 选择下 run_step 设了 `SINGULARITYENV_AGENTFLOW_*`(可断言 env 构造,不真跑容器)。
- 引擎不进缓存键:docker vs singularity 同 image → 同 `runtime_config_json`。
- 跑:`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test -p agentflow-core` + `cargo test -p agentflow-cli`(相关)、`bash scripts/acceptance-v1.sh`。**不要** `cargo test --workspace`。

## 边界(诚实)

- SingularityEngine 与 SINGULARITYENV_ 转发是**设计 + argv/env 测试**,**未对真实 singularity 跑过**(无 host)。env 转发机制(SINGULARITYENV_)是 singularity 标准做法,但真实行为待 HPC host 验证。CAPABILITIES 已记容器后端 live 未证;本切片同。
- 仅改 `crates/agentflow-core`(backend.rs/mod.rs)+ `crates/agentflow-cli`(cli_args 校验/帮助文案,若需);无新依赖。

不要 commit。报告:Singularity 枚举/引擎/argv、SINGULARITYENV_ 转发点、确认引擎不进缓存键 + 默认 docker 路径等价 + argument.rs 未动 + acceptance 绿。
