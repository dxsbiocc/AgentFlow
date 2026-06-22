# RFC: 容器执行后端 —— 硬文件系统隔离 + default-deny 出网(关 #36)

Status: Draft (设计基线)
Scope: `crates/agentflow-core/src/runtime/`(`backend.rs` trait、`run_step` 接线)、`storage`(`ToolRuntimeSpec` 镜像字段)
North star: [docs/CAPABILITIES.md](../CAPABILITIES.md) 诚实性不变量不变;`argument.rs` 0-LLM/0-网络不变;**容器只改"在哪跑",不改结果**(同输入+同镜像 → 同输出/同缓存)。
前置:P1.1 后端 trait(#70)、P1.3 I/O staging(#72,逻辑隔离)、issue36 部署级配方(`docs/ops/egress-containment.md`)。

## 1. 动机

P1.3 的 I/O staging 是**逻辑**隔离(symlink 可被 follow 到 store)。issue #36(唯一历史 open issue)要的是**反篡改硬隔离**:工具进程只能看到挂进来的 staged input、默认无网。容器后端把 P1.3 的逻辑边界升级为 OS 强制的硬边界,正式关闭 #36。它是 `ToolExecutionBackend` 的又一个实现——P1.1 的缝就是为此设计的。

## 2. 设计关键:trait 需要执行上下文

现 `ToolExecutionBackend::prepare_command(runtime) -> PreparedRuntimeCommand` 只拿 runtime spec,**拿不到 workdir / staged I/O 路径**(它们在 run_step 里)。容器后端要 `docker run -v <workdir>:... <image> <cmd>`,必须知道挂载点。故扩展 trait:

```
struct ExecContext<'a> {
    workdir: &'a Path,            // per-step 隔离 workdir(P1.3)
    staged_inputs: &'a BTreeMap<String, PathBuf>,  // port -> staged 路径(workdir 内)
    output_dir: &'a Path,         // 声明 outputs 收集处(workdir 内)
}
trait ToolExecutionBackend {
    fn prepare_command(&self, runtime, ctx: &ExecContext) -> Result<PreparedRuntimeCommand, _>;
}
```

现有 local/conda/isolated 后端忽略 `ctx`(行为不变);容器后端用它构造挂载。run_step 把已有的 workdir/staged 信息打包成 `ExecContext` 传入(P1.3 已经把 input stage 进 workdir,故挂 workdir 即覆盖声明 I/O)。

## 3. 容器后端模型

`runtime.backend: container`,新增镜像字段(`runtime.image`)。`prepare_command` 产出:

```
<container_runner> run --rm \
  --network none \                 # default-deny 出网(关 #36 的硬封堵)
  -v <workdir>:<workdir>:rw \      # 只挂 per-step workdir(staged I/O 都在内)
  -w <workdir> \
  -e AGENTFLOW_INPUT_* -e AGENTFLOW_PARAM_* -e AGENTFLOW_OUTPUT_* \
  <image@digest> <tool command...>
```

要点:
- **硬 FS 隔离**:只挂 workdir → 容器内进程看不到 artifact store / 宿主其余路径 → P1.3 逻辑隔离升为硬隔离。
- **default-deny 出网**:`--network none` 默认;需公网时(声明式 `runtime.egress: allowlist`)走受控网桥 + allowlist(对接 egress-containment.md;首切片只做 `--network none`,allowlist 留后续)。
- **镜像 digest 钉死**:`image@sha256:...` 进缓存键(`runtime_config_json` 加 `container_image`,Option + skip_serializing_if,**旧后端缓存键逐字节不变**,同 P1.2 手法)。
- **可注入 runner**(关键:离线可测,镜像 P1.2 的 provisioner 模式):`container_runner`(docker/podman 路径)经一个可注入抽象调用;测试注入 fake runner 断言构造的 docker argv,**不真跑 Docker**。

## 4. 诚实边界

- 容器后端是 #36 的**真封堵**:`--network none` + 只挂 workdir = 反篡改对手也出不去网、读不到 store。
- 但仍诚实声明:封堵强度 = 容器运行时配置正确性(operator 责任);allowlist 模式有 CDN 漂移局限(egress-containment.md 已述)。
- grade-cap 不受影响:容器里跑的未验证工具仍被 cap(后端与 maturity 正交)。

## 5. 第一切片(P-C.1,最窄可跑)

**范围:`ExecContext` trait 扩展 + `container` 后端(`--network none` + 只挂 workdir + 镜像 digest 进缓存键)+ 可注入 runner。** 不含 egress allowlist、不含 podman/apptainer(后续后端)。

交付:
1. 扩展 `ToolExecutionBackend::prepare_command` 收 `ExecContext`;local/conda/isolated 忽略它(行为等价,现有测试断言不改即过)。
2. `ContainerBackend`:构造 `docker run --rm --network none -v workdir -w workdir -e ... <image@digest> <cmd>`;经可注入 `ContainerRunner`(默认真实 docker,测试注入 fake)。
3. `ToolRuntimeSpec.image: Option<String>`(+ yaml 解析 + 校验:container 后端必须声明 image);`runtime_config_json` 加 `container_image`(Option + skip_serializing_if,旧后端字节不变)。
4. `backend_for("container")` 返回容器后端。
5. 文档:CAPABILITIES 执行隔离节 + issue #36 标记"容器后端硬封堵已落地(`--network none`)";README 能力表。

**不变量(硬约束)**:
- `git diff crates/agentflow-core/src/argument.rs` 为空。
- **容器只改在哪跑不改结果**:回归测试断言——同输入+同镜像,容器后端 vs(mock 下)产出/缓存键一致;`--network none` 在构造的 argv 中。
- 旧后端(local/conda/isolated)`runtime_config_json` 与 prepare_command 行为**逐字节不变**;现有测试不改即过。
- 不真跑 Docker(注入 fake runner);离线、低负荷。
- 无新 Rust 依赖(docker 经子进程);`unsafe_code=forbid` 不破。

## 6. 实施切分

- **P-C.1a**:`ExecContext` trait 扩展 + 三现有后端忽略 ctx(纯重构,行为等价)。← 先做,零行为变化。
- **P-C.1b**:`ContainerBackend`(`--network none` + 挂载 + image 字段 + 缓存键 + 注入 runner + 测试)。
- 后续:egress allowlist 模式、podman/apptainer 后端、容器内运行真实 live 验证(Docker 环境)。

P-C.1a 先行(P-C.1b 依赖 trait 扩展)。

## 7. 与初心 / 教训的关系

- 沿用小步可 review、每步守不变量的治理。
- **吸取本会话两次回顾教训**:不堆未验证复杂度——首切片只 `--network none`(最朴素硬封堵),allowlist/真 Docker live 验证按需再做;每片有"容器不改结果"的证据。
- 关闭 #36 把安全姿态从"合作层 + 部署配方"升到"运行时可选硬封堵",与 P1.3 staging 自然衔接。
