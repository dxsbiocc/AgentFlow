# 简报：P1.3 per-step I/O staging(Nextflow 式,组合只经声明接口)

Status: Assigned to Codex（worktree /tmp/af-p13，branch feat/p13-io-staging，从 main 起）
RFC: docs/design/isolated-execution-engine-design.md §3.3 / §6(P1.3)。前置 P1.1/P1.2 已合并。

## 现状(crates/agentflow-core/src/runtime/mod.rs)

- `run_step`(:1021)建 workdir `project_dir/work/<attempt_id>`(:1056),outputs root 在 workdir 下(:1062)。
- `resolve_inputs`(:1518)把声明 input artifact 解析成 `ResolvedInput`(指向 **artifact store 里的真实路径**)。
- `input_paths`(:1894)把这些 store 路径喂给工具(经 inputs.json + 工具读取约定 / 可能的 `AGENTFLOW_INPUT_*` env)。
- **缺口**:input 以 store 绝对路径暴露,未 stage 进 per-step workdir → 工具能看到 workdir 之外的 store 路径,工具间无文件系统隔离;组合不是严格"只经声明 I/O"。

## 目标(本切片)

把声明的 input **stage 进 per-step workdir**,工具只通过 workdir 内的 staged 路径访问输入;outputs 已在 workdir,采集声明 outputs 回 store(保持)。使"工具 A 的中间文件不被工具 B 看到,除非经声明 I/O 传递"成为结构性属性。

## 实现要求

1. **stage-in**:run_step 在 workdir 下建 `inputs/`,对每个声明 input port 把其 artifact stage 成 `workdir/inputs/<port>/<filename>`(或 `workdir/inputs/<port>` 单文件)。
   - 默认 **symlink**(轻量;跨设备/symlink 失败则 **copy** 回退)。
   - 暴露给工具的 input 路径改为 **staged 路径**(改 `input_paths` / inputs.json / 任何 `AGENTFLOW_INPUT_*` env 注入,使其指向 staged 路径而非 store 路径)。**工具透明**:工具按既有约定拿到一个有效文件路径,只是位置变到 workdir 内 → 现有工具/acceptance 必须照常工作。
2. **stage-out**:保持现有 outputs-under-workdir + 采集声明 outputs 回 store 的行为不变;只确保未声明文件不被采集(现状应已如此,确认)。
3. **缓存键**:input 身份仍按内容/hash(`input_hashes_json`)进缓存键——staging 是物理布局变化,**不得改变缓存键语义/字节**(input hash 基于内容,不基于路径;确认现状如此,保持)。staged 路径不应进缓存键。
4. **诚实边界**:本切片是 conda/local 下的**逻辑** staging(symlink 可被跟随到真实 store 路径)。**硬隔离**(只挂载 staged input、阻止跟随)随 **container 后端**到来。在代码注释 + CAPABILITIES 执行隔离节写明:P1.3 = 逻辑 staging(workdir 即边界),硬 FS 隔离 = 容器后端。

## 不变量(硬约束)

- `git diff crates/agentflow-core/src/argument.rs` 为空。
- **缓存键不变**:`runtime_config_json` 与 input/output hash 进缓存键的语义/字节不变(staging 不进缓存键)。现有精确字节缓存测试不改即通过。
- 现有 `examples/` 工具 + acceptance 全绿(工具透明拿到 staged 路径)。
- 仅改 `crates/agentflow-core/src/runtime/`(+ 可选 CAPABILITIES 文档);不碰 argument.rs / storage schema(workdir 列已存在)。
- 无新依赖;`unsafe_code=forbid` 不破。

## 测试(离线,低负荷)

- 隔离不变量:注册一个最小本地工具 + 两个导入工件(一个声明为 input,一个不声明);run 后断言 `workdir/inputs/` **只含声明的那个 port**,未声明工件不在 workdir;工具收到的 input 路径在 workdir 之内(不是 store 路径)。
- 透明性:现有 marker/survival demo 流程照跑成功(staged 路径可读,产出 outputs 正常采集)。
- symlink 回退 copy 的路径有覆盖(可在不支持 symlink 的情形或强制 copy 分支断言)。
- 现有缓存命中/恢复测试照过(input 内容不变 → 缓存键不变 → cache hit 仍成立)。
- 跑:`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test -p agentflow-core`、`bash scripts/acceptance-v1.sh`。**不要** `cargo test --workspace`。

不要 commit。报告:stage-in 实现位置、暴露给工具的路径如何改成 staged、确认缓存键未变 + 现有工具透明、隔离不变量测试、argument.rs 未动、acceptance 绿。
