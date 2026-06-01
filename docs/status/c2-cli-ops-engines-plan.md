# C2 实现简报：branch / decision / forage / trace CLI

Status: Implemented + verified (2026-06-01)
Date: 2026-05-31
Owner(orchestrator): Claude · Executor: Codex
Spec source: 统一 CLI 里程碑（第 2 片）
Depends on: H2 / H3 / H4 / H6 + C1（均已验收）

## 验收记录（Claude 独立复验 2026-06-01）

- ✅ `clippy -D warnings` 无警告；`cargo test` cli **34**（基线 26，+8）/ core 151 / schemas 3 全绿。
- ✅ 新命令在独立文件 `agent_ops_commands.rs`（43KB）；C1 的 `agent_commands.rs` 未动。
- ✅ lib.rs 仅 `mod` + 4 dispatch + usage；现有 handler/测试零删改（`git diff` 核验）。
- ✅ core 纯 additive to_json（branch 89/0、handoff 82/0、trace_guard 57/0、forage 23/0，删除行全 0）。
- ✅ 无新依赖；现有 26 cli 测试未改且通过。
- ✅ **端到端冒烟**（真实二进制，跨引擎）：`trace checkpoint → forage observe(abstract) → forage link → verdict render → branch candidates → trace drift → trace revert`。验证 §15 合规（abstract 证据落 `inconclusive_provisional` 而非 affirmed）、漂移计数、回退「已记录、不物删」。

结论：合并就绪。**统一 CLI 完成**——控制层四引擎全部能力现可手动驱动。

## 目标

把交接/轨迹/觅食/分支四组能力暴露为 CLI，补齐统一 CLI。延续 C1 的低风险 additive 模式。

## 编排者裁决（架构约束，不可违反）

1. **新命令组放进新文件** `crates/agentflow-cli/src/agent_ops_commands.rs`（C1 的 `agent_commands.rs` 不动）。lib.rs **只允许**：加 `mod agent_ops_commands;`、在顶层 match 加 `branch`/`decision`/`forage`/`trace` 四个 dispatch 分支、追加 usage 文本、必要时再把个别私有助手提 `pub(crate)`。**不得改写任何现有 handler 函数体或现有测试。**
2. 现有 **26 个 cli 测试**保持原样且全绿。
3. 不新增 crate 依赖。
4. core 仅 additive：给下列类型补 `to_json`（缺则补，不改既有签名/逻辑）：`branch::BranchCandidate`、`branch::BranchDecision`、`handoff::DecisionPoint`、`forage::ForageObservation`、`trace_guard::{Checkpoint, DriftReport, RevertRecord}`。
5. **CLI 只暴露用户侧动作**：不暴露 `raise_decision_point`（agent 内部提决策点）、不暴露 `propose_branch_patch`（需 ProposedStep，属 flow/H7）。
6. 严格复刻 C1 / 现有 CLI 约定（handler 签名 `fn xxx_command<I>(args:I)->Result<String,CliError>`、`ProjectStore::open(&path)`、`--json`/`--path`、`CliError`、stance/枚举解析复用 C1 evidence 命令的做法）。

## 交付物：`crates/agentflow-cli/src/agent_ops_commands.rs`

### `branch`
- `branch candidates [--json] [--path <p>]` → `branch_candidates()`
- `branch select [--explore] [--json] [--path <p>]` → `select_branches(&RuleBasedSelector, &BranchPolicy{ explore_enabled: <--explore 存在> })`

### `decision`（用户侧：看 + 解决）
- `decision list [--json] [--path <p>]` → `list_decision_points()`
- `decision pending [--json] [--path <p>]` → `pending_decision_points()`
- `decision show <decision-id> [--json] [--path <p>]` → `inspect_decision_point()`
- `decision resolve <decision-id> --choose <index> --note <text> [--json] [--path <p>]` → `resolve_decision_point(id, index, note)`（非法 index / 重复解决由 core 透传）

### `forage`
- `forage observe --source <s> --external-id <e> --title <t> --access <metadata_only|abstract_available|open_access_full_text|user_provided_full_text|subscription_connector_full_text|full_text_unavailable|retrieval_failed> [--json] [--path <p>]` → `record_forage_observation(...)`（access 用 `AccessStatus::parse`）
- `forage list [--json] [--path <p>]` → `list_forage_observations()`
- `forage show <forage-obs-id> [--json] [--path <p>]` → `inspect_forage_observation()`
- `forage link --hypothesis <id> --observation <forage-obs-id> --stance <supports|contradicts|neutral> --note <text> [--json] [--path <p>]` → `link_forage_evidence(...)`（grade 由 core 从 access_status 推导）

### `trace`
- `trace checkpoint --label <text> [--json] [--path <p>]` → `create_checkpoint()`
- `trace list [--json] [--path <p>]` → `list_checkpoints()`
- `trace drift <checkpoint-id> [--json] [--path <p>]` → `detect_drift()`
- `trace revert <checkpoint-id> [--json] [--path <p>]` → `revert_to()`（人类输出提示「已记录回退，N 条事件标记为回退；不物理删除」）

### lib.rs dispatch（机械添加）
顶层 match 增加 `branch`/`decision`/`forage`/`trace` 四分支；usage 追加对应用法行。

## 验收标准（Claude 审核逐条核对）

- [ ] `clippy -D warnings` 无警告；`cargo test` 全绿，cli 较 26 净增。
- [ ] 现有 26 个 cli 测试**未被修改**且通过；core 151 不变（仅 additive to_json，测试数可不变或微增）。
- [ ] 无新增依赖；lib.rs 仅 mod+4 dispatch+usage(+必要 pub(crate))，现有 handler 零改动（`git diff` 核对）。
- [ ] 7 个类型 `to_json` 为纯 additive（`git diff --numstat` 删除行=0）。
- [ ] 每组至少 1 个 happy-path + 1 个错误透传测试（如 `decision resolve` 越界、`forage observe` 非法 access、`trace drift` 不存在 checkpoint）。
- [ ] `--json` / `--path` 在新命令生效。

## 不在本里程碑（明确排除）

`raise_decision_point` / `propose_branch_patch` 的 CLI（agent/flow 内部，H7）、主循环（H7）、实际检索工具脚本、报告渲染整合。
