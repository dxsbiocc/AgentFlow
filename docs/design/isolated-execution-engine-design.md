# RFC: 隔离执行引擎 —— 每工具环境隔离 + 灵活组合 + 智能调度(v0.2.0 P1)

Status: Draft (P1 设计基线)
Scope: `crates/agentflow-core/src/runtime/`、`storage/tool_registry.rs`(`ToolRuntimeSpec`)、`crates/agentflow-cli`(`env`/`run` 命令)
North star: [docs/CAPABILITIES.md](../CAPABILITIES.md) 的诚实性不变量必须保持;`argument.rs` 判决出口 0-LLM/0-网络不变。

## 1. 愿景(用户重申的标准)

AgentFlow 的执行模型对标 Nextflow,并叠加 agent 智能:

1. **Agent 智能调度**:agent 决定/调度任务执行(不止静态 DAG 跑),调度是一等智能层。
2. **每工具环境隔离(Nextflow 式)**:每个工具在自己的隔离环境里跑,工具之间无共享可变运行时。
3. **工具灵活组合**:不同工具自由组合成流水线。
4. **输入/输出/参数 = 标准调整接口**:每个工具暴露 I/O 端口 + 参数作为统一调参面;组合与调度都通过这个接口进行。

本 RFC 覆盖到达该愿景的工程路径,并把 **v0.2.0 P1 第一切片**裁成"隔离后端 + I/O staging"(最窄可跑、先证机制)。

## 2. 现状(已具备的基础,勿重造)

代码已天然贴合愿景,P1 是**强化**而非推倒:

- **统一接口已在**:`ExecutableToolSpec`(`tool_registry.rs:71`)已暴露 `inputs` / `outputs`(`ToolPortSpec`)+ `params`(`ToolParamSpec`)。这就是用户要的"标准调整接口",无需新建。
- **后端分发已在**:`run_step`(`runtime/mod.rs:891`)与 `prepare_tool_environment`(`:276`)用 `match backend { "local" | "conda" | "micromamba" }` 分发。`ToolRuntimeSpec`(`:61`)带 `backend/command/timeout/env_name/env_prefix/env_file/runner`。
- **DAG + needs 门已在**:`run_flow`(`:495`)、`run_step_ref`(`:672`)、`ensure_step_dependencies_completed`、`run_step_ref_executes_only_selected_ready_step`(`:4121`)——拓扑就绪门已经在跑。
- **缓存已含运行时身份**:`runtime_config_json`(`:1770`)把 `backend/command/env_*/env_file_hash/runner` 折进缓存键。
- **env 证据链已在**:`env check|prepare|export`(`prepare_tool_environment`、`export_tool_environment`)+ 导出哈希 + 包集 diff。
- **隔离 workdir 雏形已在**:`isolated_workdir`(`synth_commands.rs:2533`)——目前仅用于 synth 验证。

**关键缺口**:`prepare_tool_environment` 只对**已存在**的 conda/micromamba prefix 做 `env update`;它**不**自动创建、不**锁定**、不保证**可复现**的隔离环境;run_step 也未把"每步隔离 workdir + 仅通过声明 I/O 组合"作为强不变量(工具间文件系统未严格隔离)。

## 3. 目标模型

### 3.1 可插拔执行后端 trait

把 `match backend` 重构为一个后端抽象:

```
trait ToolExecutionBackend {
    fn prepare(&self, tool, env_spec) -> PreparedEnv;     // 解析/创建/锁定隔离环境
    fn run(&self, prepared, staged_workdir, argv, limits) -> ExecOutput;  // 在隔离环境内执行
    fn identity(&self) -> BackendIdentity;                // 进缓存键(后端 + 环境锁哈希)
}
```

实现序列(用户已定优先级):
- **P1:`isolated-conda` / `isolated-micromamba`**(本切片)——自动创建 + 锁定 + 复现的 per-tool env。
- P2:`container`(Docker/OCI)——镜像 digest 钉死;default-deny egress(关 #36)。
- 后续:Podman(rootless)、Apptainer/Singularity(HPC)。

`local` 后端保留(无隔离的直跑),但文档标注为最弱隔离。

### 3.2 每工具隔离环境(Nextflow 式 process == env)

- 从 `env_file`(声明依赖)**自动创建**一个受管 per-tool 环境到 `.agentflow/envs/<tool>@<lockhash>/`,不再要求预先存在 `env_name`。
- **锁定**:solve 后导出显式锁(`micromamba env export --explicit --md5` 或等价),记录 `env.lock`;环境**内容寻址**于 lock 哈希。
- **复现**:相同 lock 哈希 → 复用既有 env;不存在则从 lock 确定性重建。
- **缓存正确性**:lock 哈希进 `runtime_config_json` → 环境变了缓存自然失效。
- 工具之间**永不共享同一个可变 prefix**:隔离单位 = (tool, lock)。

### 3.3 I/O staging(组合只经声明接口)

- 每步在 per-step 隔离 workdir(`.agentflow/runs/<run>/<step>/`)里跑。
- **stage-in**:把该步声明的 input artifact 暂存进 workdir(只读/符号链接或拷贝),按 `inputs` 端口名映射到固定路径。
- **stage-out**:运行后只采集声明的 `outputs` 端口产物回 artifact store;workdir 其余内容丢弃。
- 效果:工具 A 的中间文件**不可能**被工具 B 看到,除非经声明 I/O 传递 → 这正是"灵活组合 + 隔离"的本质,也消除隐式 FS 耦合。

### 3.4 智能调度(P2+,本切片不做)

- P1 保持现有拓扑就绪门(`ensure_step_dependencies_completed`)。
- P2:控制层按就绪集 + 优先级智能选择下一步(已有 `run_step_ref_executes_only_selected_ready_step` 作为单步执行原语);并行调度 + 取消另立。
- agent 调度建立在 §3.3 的隔离 + §3.1 的后端身份之上 —— 先有隔离与可复现,调度才安全。

## 4. v0.2.0 P1 第一切片(最窄可跑)

**范围:隔离后端 trait + `isolated-micromamba`(或 conda)一个实现 + per-step I/O staging + 环境锁进缓存键。** 不含 container、不含 egress、不含 agent 调度(留 P2)。

交付:
1. `ToolExecutionBackend` trait + 把现有 `local`/`conda`/`micromamba` 分发迁到 trait 之后(行为等价,纯重构 + 测试护住)。
2. 新后端 `isolated-micromamba`:`env_file` → 自动创建 `.agentflow/envs/<tool>@<lockhash>/` + 导出 `env.lock`(显式锁)+ 复现复用;lock 哈希进 `runtime_config_json`。
3. per-step I/O staging:run_step 在隔离 workdir stage-in 声明 inputs、stage-out 声明 outputs;断言工具不能读未声明路径(隔离测试)。
4. CLI:`env prepare` 支持"从 env_file 创建并锁定"(而非仅 update);`tools inspect` / run 报告显示后端 + lock 哈希。
5. 文档:CAPABILITIES 增"执行隔离"节;README 能力表更新。

**不变量(硬约束)**:
- `git diff crates/agentflow-core/src/argument.rs` 为空;判决确定性不变。
- 缓存键正确反映环境(lock 哈希);相同 lock 复现复用,不同 lock 失效。
- 隔离不改 grade-cap:工具 maturity 与后端无关(隔离环境跑出的未验证工具仍被 cap)。
- 受管 env / 锁 / workdir 全在 `.agentflow/` 下,不入仓库;不把单次产物当核心。
- 无新 Rust 依赖(env 操作走子进程调 micromamba/conda);现有 `examples/` + acceptance 全绿。

## 5. 风险与开放问题

- **solve 联网**:创建 env 需下载包(联网)。这属工具环境层,不碰判决路径;离线/受限网络环境需 P2 容器 + 预置镜像或本地 channel。文档诚实标注。
- **micromamba 可用性**:`isolated-*` 后端要求 `runner` 指向 micromamba/conda;不可用时 `env check` 明确报错,`local` 后端仍可用。
- **跨平台锁**:显式锁是平台相关的(linux-64 vs osx-arm64);lock 哈希要带平台标签,避免跨平台误复用。
- **大型 env solve 耗时/资源**:与本机低负荷约束相关 —— 默认不在 CI 跑真实 solve;测试用 mock runner / 注入式 fake backend 断言编排逻辑,真实 solve 留集成/手动。

## 6. 实施切分(给后续 codex 苦力活)

- **P1.1**:后端 trait 抽象 + 现有三后端迁移(纯重构,行为等价,测试护住)。← 先做这个,零行为变化、零风险,给后面铺路。
- **P1.2**:`isolated-micromamba` 后端(自动创建 + 锁 + 复现 + lock 哈希进缓存键)。
- **P1.3**:per-step I/O staging + 隔离不变量测试。
- **P1.4**:CLI/报告/文档收尾。

每个 P1.x 是一个可独立 review/合并的 PR;P1.1 先行(其它依赖它)。P1.2/P1.3 在 P1.1 合并后可并行(不同文件)。

## 7. 与既有不变量/边界的关系

- 复用 [tool-evolution-engine RFC](./tool-evolution-engine-design.md) 的治理风格:小步、可 review、每步守 §8 不变量。
- 隔离后端是后续 **container 后端(关 issue #36 真封堵出网)** 的地基:先有 trait + I/O staging,容器实现只是再加一个 `ToolExecutionBackend`。
- agent 智能调度(愿景 §1)建立在本 RFC 的隔离 + 可复现之上,作为 v0.2.0 之后的 P2。
