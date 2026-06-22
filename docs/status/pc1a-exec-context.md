# 简报：P-C.1a 后端 trait 加 ExecContext(纯扩展,行为等价)

Status: Assigned to Codex（worktree /tmp/af-pc1a，branch feat/pc1a-exec-context，从 main 起）
RFC: docs/design/container-backend-design.md §2 / §6(P-C.1a)。为容器后端铺路:让后端 prepare_command 能拿到 workdir / staged I/O。

## 现状

- `ToolExecutionBackend::prepare_command(&self, runtime) -> PreparedRuntimeCommand`(`runtime/backend.rs`)只拿 runtime spec。
- `run_step`(`runtime/mod.rs:1027`)在 `:1071` 调 `prepare_runtime_command_for_tool(&tool.runtime, isolated_env.as_ref())`,此处已有:`workdir`(:1061-1063)、`resolved_input_paths`(:1064,staged port→PathBuf)、`resolved_outputs.root()`(:1068,输出目录)。这些**还没传进后端**。

## 目标(纯扩展,行为等价)

给 `ToolExecutionBackend::prepare_command` 加一个 `ExecContext` 参数;现有 local/conda/isolated-micromamba 后端**忽略它**(行为不变)。run_step 用既有 workdir/staged 数据构造 ExecContext 传入。为后续容器后端(用 ctx 构造挂载)铺缝。

## 实现要求

1. `runtime/backend.rs` 定义:
   ```
   pub(super) struct ExecContext<'a> {
       pub workdir: &'a std::path::Path,
       pub staged_inputs: &'a BTreeMap<String, PathBuf>,  // port -> staged 路径(workdir 内)
       pub output_dir: &'a std::path::Path,
   }
   ```
   trait 改:`fn prepare_command(&self, runtime: &ToolRuntimeSpec, ctx: &ExecContext) -> Result<PreparedRuntimeCommand, StorageError>;`
2. 现有三后端(Local/Conda/IsolatedMicromamba)impl 签名加 `_ctx: &ExecContext`,**不使用**(行为字节级不变)。
3. `prepare_runtime_command`(及 `prepare_runtime_command_for_tool`)签名加 `ctx`,透传给后端;run_step 在 :1071 处构造真实 ExecContext(workdir=&workdir、staged_inputs=&resolved_input_paths、output_dir=resolved_outputs.root())传入。
4. 对**没有 staging 上下文的调用点(单测里直接调 prepare_runtime_command 的)**:提供一个 `ExecContext` 的最小构造(如 `ExecContext::for_tests()`/一个指向空 workdir 的 dummy,或让测试构造一个临时 dir + 空 map)。目标是现有断言**不变**仍通过——加 ctx 参数是机械改动,不改任何 argv/错误文案/缓存键。

## 不变量(硬约束)

- `git diff crates/agentflow-core/src/argument.rs` 为空。
- **行为等价**:现有所有测试(run_flow_*、prepare_runtime_command 等价测试、isolated 缓存键等)**断言不改**即通过——现有后端忽略 ctx,argv/错误/缓存键逐字节不变。
- 仅改 `crates/agentflow-core/src/runtime/`(backend.rs + mod.rs);不碰 storage schema / CLI / argument.rs。无新依赖。
- 不引入容器逻辑(那是 P-C.1b);本片只搬 trait 形状。

## 测试(离线,低负荷)

- 现有测试原样通过(等价证明)。
- 可加一个小测:Local/Conda 后端在给定任意 ExecContext 下 argv 与之前一致(忽略 ctx 的证据)。
- 跑:`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test -p agentflow-core`、`bash scripts/acceptance-v1.sh`。**不要** `cargo test --workspace`。

不要 commit。报告:ExecContext 定义、trait/调用点改动、run_step 构造点、确认现有测试未改即过 + argument.rs 未动 + acceptance 绿。
