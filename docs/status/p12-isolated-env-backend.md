# 简报：P1.2 isolated-micromamba 后端(内容寻址隔离 env + 锁 + 缓存键)

Status: Assigned to Codex（worktree /tmp/af-p12，branch feat/p12-isolated-env，从 main 起）
RFC: docs/design/isolated-execution-engine-design.md §3.2 / §4 / §6(P1.2)。前置 P1.1 已合并(`runtime/backend.rs` 的 `ToolExecutionBackend` trait + `backend_for`)。

## 目标

新增后端 `isolated-micromamba`(可顺带 `isolated-conda`):从工具声明的 `env_file` **自动创建**一个**内容寻址**的 per-tool 隔离环境到 `.agentflow/envs/<tool>@<lockhash>/`,导出锁,复现复用,并把环境身份折进缓存键。**第一切片用可注入的 env-creator,使编排逻辑离线可测,不在测试里跑真实 micromamba solve(本机低负荷)。**

## 现状(可复用)

- `ToolExecutionBackend`(`runtime/backend.rs`)+ `backend_for`(P1.1)。
- `runtime_config_json`(`runtime/mod.rs:1770`)已把 `backend/command/env_*/env_file_hash/runner` 折进缓存键。
- 既有 `prepare_tool_environment`(:288)对**已存在** prefix 做 env update —— isolated 后端不同:它**创建并锁定**受管 prefix。
- `CondaBackend`(`run -p <prefix> ...`)的命令构建可复用给 isolated(指向受管 prefix)。

## 实现要求

### 1. 内容寻址受管 env
- 定义 `lockhash = fnv64( env_file 内容 + platform_tag )`(platform_tag 如 `osx-arm64`/`linux-64`,用 `std::env::consts::OS/ARCH` 或现有工具;**必须**带平台,避免跨平台误复用)。
- 受管 prefix:`<project>/.agentflow/envs/<tool_id>@<lockhash>/`(tool_id 用现有 `tool_id(namespace,name)` 风格,做路径安全清洗)。
- 复用:prefix 已存在(且含 `agentflow-env.lock` 完成标记)→ 跳过创建直接用;否则创建。

### 2. 可注入 env-creator(关键:离线可测)
- 定义一个小 trait/函数指针,如 `trait IsolatedEnvProvisioner { fn ensure(&self, env_file: &Path, prefix: &Path, runner: &str) -> Result<(), StorageError>; }`。
- 真实实现 `MicromambaProvisioner`:`<runner> create -y -p <prefix> -f <env_file>` 然后 `<runner> env export -p <prefix> --explicit > <prefix>/agentflow-env.lock`(命令细节以可用为准),`env_clear()` + 仅 PATH(与既有 env 卫生一致)。
- 编排逻辑(prefix 派生、复用判断、lockhash、缓存键)与 provisioner **解耦**,测试注入一个 fake provisioner(只 `mkdir prefix + 写 lock`,不联网)。

### 3. 接进后端 + 缓存键
- `backend_for("isolated-micromamba")` 返回 isolated 实现;其 `prepare_command` 用受管 prefix 走 `run -p <prefix> <command>`(复用/参数化 CondaBackend)。
- isolated 后端在 run 前 `ensure` 受管 env(provisioner)。把 **lockhash** 折进 `runtime_config_json`(新增字段如 `isolated_env_lock`,仅 isolated 后端非空)→ env 变了缓存失效;相同 env 复现复用。**不要改既有 local/conda/micromamba 的缓存键输出**(向后兼容:新字段对旧后端为 null/缺省,且必须保证既有缓存键字节不变 —— 若加字段会改 JSON,请用 `#[serde(skip_serializing_if)]` 或仅对 isolated 注入,确保旧后端 `runtime_config_json` 输出与今天逐字节一致)。
- CLI:`env prepare` 对 isolated 后端执行"创建+锁定"(而非 update);`tools inspect`/run 报告显示后端 + lockhash(可选,能加则加)。

### 4. 不变量(硬约束)
- `git diff crates/agentflow-core/src/argument.rs` 为空。
- **既有 local/conda/micromamba 的 `runtime_config_json` 输出逐字节不变**(回归:保留现有精确字节断言 `runtime_config_json` 测试不改并通过)。
- 受管 env / 锁全在 `.agentflow/` 下;`.gitignore` 已含 `.agentflow/`(确认;若否则加)。不把单次产物当核心。
- grade-cap 不受影响(隔离 env 跑出的未验证工具仍被 cap)。
- 无新依赖;`unsafe_code=forbid` 不破;不在测试里跑真实 micromamba(注入 fake)。

## 测试(离线,低负荷)

- 单测:lockhash 内容寻址(同 env_file+platform→同 prefix;改 env_file→变 prefix;平台标签在 hash 内);复用(prefix+lock 已存在→fake provisioner 不被调用);isolated `prepare_command` 用 `run -p <managed_prefix>`;`runtime_config_json` 对 isolated 含 lockhash、对 local/conda **逐字节不变**。
- 现有测试断言不改即通过。
- 跑:`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test -p agentflow-core`、`bash scripts/acceptance-v1.sh`。**不要** `cargo test --workspace`,**不要**跑真实 conda/micromamba solve。

不要 commit。报告:新增后端/provisioner/lockhash 的位置、缓存键改动如何保证旧后端字节不变、新增测试、argument.rs 未动、acceptance 绿。
