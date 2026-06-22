# 简报：P1.1 执行后端 trait 抽象(纯重构,行为等价)

Status: Assigned to Codex（worktree /tmp/af-p11，branch feat/p11-backend-trait，从 main 起）
RFC: docs/design/isolated-execution-engine-design.md §3.1 / §6(P1.1)。
目标:把现有按 `runtime.backend` 字符串分发的逻辑收敛到一个 `ToolExecutionBackend` trait 之后,为后续 isolated / container 后端铺路。**纯重构:行为字节级等价,不改任何外部可见行为、不改缓存键、不改命令/错误文案。**

## 现状(分发点,crates/agentflow-core/src/runtime/mod.rs)

`match tool.runtime.backend.as_str()`(及等价)出现在:
- `:246` `check_tool_environment` —— `local` vs `conda|micromamba`。
- `:288` `prepare_tool_environment` —— 同上。
- `:391` `export_tool_environment` —— 同上。
- `:1843` 运行命令构建辅助(run_step 经它构造执行命令)—— `local` vs `conda|micromamba`,且 `:1860`、`:2328` 有 `backend == "conda"` 的细分支(conda 与 micromamba 的细微差异)。

`runtime_config_json`(`:1770`)把 backend/command/env_* 折进缓存键 —— **本次不要改它的输出**(缓存键保持不变)。

## 重构要求

1. 定义 `trait ToolExecutionBackend`,方法覆盖当前按 backend 分支的操作,建议:
   - `fn check_env(&self, runtime, items: &mut Vec<EnvironmentCheckItem>)`(或返回 items)
   - `fn prepare_env(&self, runtime) -> ...`(对应 prepare 的命令构建/执行编排)
   - `fn export_env(&self, runtime) -> ...`
   - `fn build_run_command(&self, ...) -> ...`(对应 :1843 的命令构建,含 conda/micromamba 细分)
   - 方法签名以"最小改动迁移现有代码"为准,可按现有内部函数边界裁剪;不强求和上面字面一致。
2. 实现:
   - `LocalBackend`(`local` 分支)
   - 一个覆盖 `conda` 与 `micromamba` 的实现(二者共享绝大多数逻辑,仅 `:1860`/`:2328` 处按 runner 类型细分)—— 用一个带 `runner_kind` 字段的实现 + 内部判断,**保留原有 conda/micromamba 差异**。
   - 不支持的 backend 字符串:保持原有"unsupported runtime backend"错误路径不变。
3. 工厂:`fn backend_for(backend: &str) -> Option<Box<dyn ToolExecutionBackend>>`(或枚举分发);各调用点改为先取 backend 再委托,删除内联 `match`。
4. **行为等价**:迁移后,所有现有路径产生**完全相同**的 argv、错误文案、env_clear/PATH 设置、超时处理、缓存键。不新增/不改变任何用户可见输出。

## 不变量(硬约束)

- `git diff crates/agentflow-core/src/argument.rs` 为空;判决出口不动。
- `runtime_config_json` 输出不变(缓存键稳定);不改 `ToolRuntimeSpec` 字段。
- 仅改 `crates/agentflow-core/src/runtime/mod.rs`(若 trait 放独立模块如 `runtime/backend.rs` 亦可,但仅限 runtime 模块内;`mod.rs` 该 `mod backend;`)。不碰 CLI、不碰 storage(除非纯 re-export,尽量不动)。
- 无新依赖;`unsafe_code = forbid` 不破。
- 所有现有测试**不改断言**即通过(行为等价的证据)。

## 测试(离线,低负荷)

- 不改现有测试断言;它们必须原样通过(等价性证明)。
- 新增:`backend_for` 对 `local`/`conda`/`micromamba` 返回对应实现、对未知返回 None(或等价路由测试);一个断言"经 trait 构建的 run 命令与重构前预期 argv 一致"的单测(可对一个最小 local 工具断言 argv)。
- 跑:`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test -p agentflow-core`、`bash scripts/acceptance-v1.sh`。**不要** `cargo test --workspace`(控制本机负荷)。

不要 commit(编排者来提交)。报告:新增 trait/impl/工厂的位置、迁移了哪些 match 点、确认所有现有测试未改断言即通过、argument.rs 未动、acceptance 绿。
