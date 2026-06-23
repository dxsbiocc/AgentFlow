# 简报：MC.3 容器工具 image-only + 多引擎文档 + 示例(收尾)

Status: Assigned to Codex（worktree /tmp/af-mc3，branch feat/mc3-image-only，从 main 起）
RFC: docs/design/multi-engine-container-design.md §3.1 / §5(MC.3)。前置 MC.1/MC.2a/MC.2b 已合并。收尾:让容器工具**只声明 image**(engine+runner 由 run profile 提供),并文档化 + 给示例。

## 现状

- tool_registry 校验(`"container" =>`,tool_registry.rs:1665):container 工具**要求 `runner`**(绝对路径)+ image。
- `ContainerBackend.prepare_command`(backend.rs)已解析:runner = `ctx.container_engine` 的 override > `runtime.runner` > "no runner" 错误(MC.2a)。所以 runner 已可由 run config 提供。

## 目标(完成 Nextflow 式:工具 image-only,引擎/runner per-run)

### 1. 放宽校验:container 工具 image-only
- tool_registry `"container"` 分支:**`image` 必需**;**`runner` 改为可选**(若声明则仍校验绝对路径;不声明则合法)。仍拒 env_name/env_prefix/env_file。
- **更新现有断言**:那个断言"container 必须声明 runner"的校验测试要改为"container 必须声明 image;runner 可选"——这是**有意的模型演进**(非静默破坏),在测试里注释说明。
- 运行期:runner = `--container-runner` override > tool `runner` > 清晰错误(已有,确认错误文案提示"pass --container-runner or declare runtime.runner")。image 仍必需(缺则现有 "container runtime must declare image" 错误)。

### 2. 文档(Nextflow 式多引擎)
- `docs/CAPABILITIES.md` §6.6:补"多引擎"——image 由工具定(per-tool 稳定),engine(docker/podman/singularity/apptainer)由 run 选(`--container-engine`/`--container-runner`,默认 docker),引擎不进缓存键(同 image 任何引擎同结果)。重申 live 未证边界。
- `README.md` 能力表:加"多引擎容器(docker/podman/singularity),引擎按 run 选,image 按工具定"。

### 3. 示例(illustrative,非 live)
- `examples/tools/<name>.tool.yaml`:一个 **image-only** 容器工具(`backend: container` + `image: <公开镜像 ref>` + 简单 command + 声明 inputs/outputs/params;**无 runner**)。镜像选一个常见公开 ref(如 `docker://ubuntu:24.04` 或一个 biocontainer),command 用镜像内已有工具(如 `cat`/`grep`)读 staged input 写 output——保持工具自包含于镜像。
- README/CAPABILITIES 给一段**引擎切换示例**:同一工具 `--container-engine docker`(本地)vs `--container-engine singularity --container-runner /usr/bin/singularity`(HPC)。诚实标注:示例需真实引擎 + 镜像才能跑,本仓库未 live 验证。

## 不变量(硬约束)

- `git diff crates/agentflow-core/src/argument.rs` 为空。
- 默认 docker 路径行为不变;非容器后端不受影响;现有缓存键测试不改即过。
- **唯一有意改的断言**:container "requires runner" → "requires image; runner optional"(注释说明模型演进)。其余现有断言不改即过。
- 仅改 `crates/agentflow-core`(tool_registry.rs)+ docs + examples;无新依赖;无 CLI 行为改动(flag 已在 MC.2a/b)。
- 示例工具是 example 工件,非 core;不真跑 Docker。

## 测试(离线,低负荷)

- container 工具**无 runner** 可注册(image-only);有 runner 仍校验绝对路径;缺 image 仍报错。
- 运行期 runner 解析:override > tool runner > 错误(可复用/扩展现有测试)。
- 示例 tool.yaml 能 `tools register`(解析通过)。
- 跑:`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test -p agentflow-core` + `cargo test -p agentflow-cli`(相关)、`bash scripts/acceptance-v1.sh`。**不要** `cargo test --workspace`。

不要 commit。报告:校验放宽点、改了哪个 runner-required 断言、文档/示例位置、确认 argument.rs 未动 + 默认路径不变 + acceptance 绿。
