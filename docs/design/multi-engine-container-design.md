# RFC: Nextflow 式多引擎容器执行(image 与引擎解耦)

Status: Draft (设计基线)
Scope: `crates/agentflow-core/src/runtime/`(`backend.rs` 容器引擎、`run_step` 引擎解析)、`storage`(`ToolRuntimeSpec`)、`crates/agentflow-cli`(run/agent 引擎选择)、project config
North star: [docs/CAPABILITIES.md](../CAPABILITIES.md) 诚实性不变量不变;`argument.rs` 0-LLM/0-网络不变;**引擎只改"用什么跑容器",不改结果**(同 image → 同输出/同缓存,无论 docker/singularity/podman)。
前置:容器后端(#80,关 #36)、ExecContext(#79)。

## 1. 动机

容器后端(#80)目前把**引擎写死在工具里**:`backend: container` + `runner: /usr/bin/docker`。要让同一工具在本地用 docker、在 HPC 用 singularity,得改工具定义——这**不是** Nextflow 式。

Nextflow 把两件事分开:
- **image**:per-process(`container 'biocontainers/x'`),工具的稳定属性。
- **engine**:docker/singularity/podman,由 **profile/config** 选,不写在 process 里。

→ 同一 pipeline 切 profile 即可换引擎,工具定义不动。本 RFC 把 AgentFlow 演进到这个模型。

## 2. 现状(clean slate)

- 后端:local / conda / micromamba / isolated-micromamba / **container**。
- `ContainerBackend`(backend.rs)用 `runtime.runner`(docker 路径)+ `runtime.image`,argv 是 docker 风格(`run --rm --network none -v … <image> <cmd>`)。
- **无任何 example/集成工具用 `backend: container`**(只有单测)→ **模型可自由演进,无向后兼容包袱**。

## 3. 目标模型:WHAT(per-tool)/ HOW(per-run)分离

### 3.1 工具只声明 image(per-tool,稳定)
- `runtime.backend: container` + `runtime.image: <ref>`。
- 工具**不再声明引擎/runner**(那是 run 期的事)。`image` 在(可移植引擎间共享:docker ref 如 `biocontainers/x:1.2`;singularity 可 `docker://` 拉取或用 `.sif`)。

### 3.2 引擎由 run profile 选(per-run/project)
- 引擎选择 = `{ engine: docker|podman|singularity|apptainer, runner_path: <绝对路径> }`。
- 来源优先级:run flag(`run/agent --container-engine <e> --container-runner <path>`)> project config(`.agentflow/` 内)> 默认 `docker`。
- 一处配置,作用于该 run 的所有 container 工具(像 Nextflow profile)。

### 3.3 `ContainerEngine` 抽象(引擎特定 argv)
```
trait ContainerEngine {
    fn build(&self, image, command, ctx: &ExecContext) -> PreparedRuntimeCommand;
}
```
- `DockerEngine`(= podman,CLI 兼容):`run --rm --network none -v wd:wd -w wd -e NAME... <image> <cmd>`(即现 ContainerBackend 逻辑,行为等价)。
- `SingularityEngine`(= apptainer):`exec --containall --net --network none -B wd:wd --pwd wd --env NAME=... <image> <cmd>`(注意:singularity 默认挂 $HOME/$PWD,需 `--containall` 关掉以达到等价隔离;env 用 `--env`/`SINGULARITYENV_`)。
- `ContainerBackend.prepare_command` 改为:从 run 期解析的引擎选择拿 `ContainerEngine`,委托 `build`。

### 3.4 缓存键(关键诚实属性)
- `image` **进**缓存键(已是,#80)。
- **引擎不进缓存键**:同 image 在 docker vs singularity 应得**相同结果**(image 内容决定结果,引擎只决定"用什么跑")。这与既有"容器只改在哪跑不改结果"一致——引擎是更强的同一原则。回归测试断言:切引擎,缓存键/结果不变。

## 4. 不变量(硬约束)

- `git diff crates/agentflow-core/src/argument.rs` 为空。
- **引擎只改 where,不改 what**:切 docker↔singularity,工具 image/命令/缓存键不变;回归测试守。
- 旧后端(local/conda/isolated)与现有 container argv:DockerEngine 必须与 #80 的 docker argv **逐字节一致**(纯重构,现有 container argv 测试不改即过)。
- `prepare_command` 仍纯 argv 构造 → 离线断言可测,**不真跑容器**。
- 无新依赖;`unsafe_code=forbid` 不破。

## 5. 实施切分

- **MC.1**:引入 `ContainerEngine` 抽象 + `DockerEngine`(= 现 ContainerBackend argv,行为等价);引擎选择先只支持 docker(默认),从 run 期解析(先用 per-tool `runner` 作为 runner_path 来源,保持 #80 行为)。**纯重构,零行为变化**。← 先做。
- **MC.2**:引擎选择上浮为 run/project config(`--container-engine`/`--container-runner` + 默认),工具 `image`-only;`SingularityEngine` + podman。回归测试:切引擎结果/缓存键不变。
- **MC.3**:docs(Nextflow 式多引擎)、CAPABILITIES、一个 container 示例工具 + 引擎选择示例。
- 后续:真实 host live 验证(docker + singularity,需 host)。

MC.1 先行(MC.2/3 依赖引擎抽象)。

## 6. 与初心 / 诚实边界

- 沿用小步可 review、每步守不变量。
- **诚实**:所有容器引擎仍是 **argv 离线测试**,**未对真实 daemon/HPC 跑过**(CAPABILITIES §6.6 已记)。多引擎不改变这一状态——live 验证 docker/singularity 需各自 host,属后续。不宣称"生产就绪"多容器,只说模型对齐 Nextflow + 命令构造正确。
- isolated-micromamba 同样 live 未证(fake provisioner);conda 多引擎与之正交。
