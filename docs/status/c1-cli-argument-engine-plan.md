# C1 实现简报：论证引擎 CLI（hypothesis / evidence / verdict）

Status: Implemented + verified (2026-05-31)
Date: 2026-05-31
Owner(orchestrator): Claude · Executor: Codex
Spec source: 统一 CLI 里程碑（第 1 片，论证引擎）
Depends on: H1 / H5（hypothesis.rs / argument.rs，已验收）

## 验收记录（Claude 独立复验 2026-05-31）

> 注：首次分派在上个会话结束时被 kill（仅写了一半 to_json），已回退重跑成功。

- ✅ `clippy -D warnings` 无警告；`cargo test` cli **26**（基线 22，+4）/ core 151 / schemas 3 全绿。
- ✅ 新命令在独立模块 `agent_commands.rs`；lib.rs **零删除**——仅 `mod` + 3 dispatch + usage 文本 + `next_arg`/`require_value` 提 `pub(crate)`；现有 handler/测试未改（`git diff` 核验 hunk 仅落在允许位置）。
- ✅ core 纯 additive：`Hypothesis`/`EvidenceLink`/`VerdictReport`/`VerdictSummary` 补 `to_json`（argument +99/-0、hypothesis +27/-0）。
- ✅ 无新依赖（Cargo 零变更）。
- ✅ **端到端冒烟**（真实二进制）：`init → hypothesis create → list → evidence link → verdict render` 跑通；无 gate 时强判决被 H5 闸门拒绝，带 gate 时产出 `affirmed/medium` + 完整 rationale。`--json` / `--path` 生效。

结论：合并就绪。branch/decision/forage/trace 的 CLI 在 C2。

## 目标

把 H1+H5 的「假设 → 证据 → 判决」闭环暴露为 CLI 命令，让用户**第一次能实际跑控制层**。这是统一 CLI 的第 1 片；branch/decision/forage/trace 在 C2。

## 编排者裁决（架构约束，不可违反）

1. **新命令组放进新模块文件** `crates/agentflow-cli/src/agent_commands.rs`，`mod agent_commands;` 在 lib.rs 声明。lib.rs **只允许**这些改动：加 `mod` 声明、在顶层 match 加 `hypothesis`/`evidence`/`verdict` 三个 dispatch 分支、把 `agent_commands.rs` 需要的私有助手（如 `next_arg`、`CliError`、`parse_*` option 助手、共享 OsString 处理）提升为 `pub(crate)`。**不得改写现有任何 handler 函数体或现有测试。**
2. 现有 22 个 cli 测试必须保持原样且全绿。
3. 不新增 crate 依赖。
4. core 侧仅允许 additive 改动：给缺失的类型补 `to_json`（见下），不改既有方法签名/逻辑。
5. 严格复刻现有 CLI 约定（参照 `research_command` / `research_note_command` / `research_list_command` / `research_inspect_command` / `observe_command`）：
   - handler 签名 `fn xxx_command<I>(args: I) -> Result<String, CliError> where I: IntoIterator<Item = OsString>`
   - store 打开 `agentflow_core::storage::ProjectStore::open(&path)?`，path 默认 `std::env::current_dir()?`，`--path <p>` 覆盖
   - `--json` 输出走 `to_json`；人类输出为多行格式串
   - 错误用 `CliError::InvalidArgument` / 透传 core `StorageError`

## core 侧 additive 改动

给以下类型补 `to_json(&self) -> String`（若已存在则复用，勿重复）：
- `hypothesis::Hypothesis`
- `argument::EvidenceLink`（若 argument.rs 已有 to_json 即复用）
- `argument::VerdictSummary`（H2 加的判决摘要）
- `argument::VerdictReport`（render 输出，至少含 hypothesis_id / verdict / confidence / rationale）

## 交付物：`crates/agentflow-cli/src/agent_commands.rs`

### `hypothesis` 命令组
- `hypothesis create --statement <s> --origin <o> --goal <goal-id> [--json] [--path <p>]` → `record_hypothesis`
- `hypothesis list [--json] [--path <p>]` → `list_hypotheses`
- `hypothesis show <hypothesis-id> [--json] [--path <p>]` → `inspect_hypothesis`
- `hypothesis transition <hypothesis-id> --to <status> [--confidence low|medium|high] [--json] [--path <p>]` → `transition_hypothesis`
  - `<status>` 解析用 `HypothesisStatus::parse`；非法状态/非法跃迁 → core 返回 `InvalidInput`，CLI 透传。

### `evidence` 命令组
- `evidence link --hypothesis <id> --grade <observed|inferred|literature_supported|hypothesis|unsupported> --stance <supports|contradicts|neutral> --note <text> [--observation <obs-id>] [--source <text>] [--json] [--path <p>]` → `link_evidence`
- `evidence list --hypothesis <id> [--json] [--path <p>]` → `evidence_for`

### `verdict` 命令组
- `verdict render --hypothesis <id> [gate 选项见下] [--json] [--path <p>]` → 用 `RuleBasedEngine` 调 `render_verdict(id, &RuleBasedEngine, gate)`
  - gate 选项（**任一 gate 选项出现则视为提供 gate，须凑齐必填**）：`--gate-supports <t>` `--gate-against <t>` `--gate-alternatives <t>` `--gate-data-risks <t>` `--gate-assumptions <t>` `--gate-falsifier <t>` `--gate-claim-basis <observed|inferred|speculative>` `--gate-not-yet <t>`
  - 无任何 gate 选项 → 传 `None`（Provisional 判决可成功；强判决会被 core 闸门拒绝并由 CLI 透传错误，提示需要 gate）。
- `verdict show --hypothesis <id> [--json] [--path <p>]` → `latest_verdict_for`（无判决时人类输出友好提示，json 输出 `null` 或空）。

### lib.rs dispatch（机械添加）
顶层 match 增加：`"hypothesis" => agent_commands::hypothesis_command(args)`、`"evidence" => agent_commands::evidence_command(args)`、`"verdict" => agent_commands::verdict_command(args)`；并在 `usage()` 文本追加对应用法行。

## 验收标准（Claude 审核逐条核对）

- [ ] `cargo clippy --workspace --all-targets -- -D warnings` 无警告；`cargo test --workspace` 全绿。
- [ ] 现有 22 个 cli 测试**未被修改**且通过；cli 测试数较 22 净增（新命令测试）。
- [ ] 无新增依赖。
- [ ] lib.rs 改动仅限：mod 声明 + 3 个 dispatch 分支 + usage 文本 + 必要的 `pub(crate)` 可见性提升；现有 handler 函数体零改动（`git diff` 核对）。
- [ ] 新命令 `--json` 与 `--path` 均生效，有测试。
- [ ] `hypothesis transition` 非法跃迁、`evidence link` 缺必填、`verdict render` 强判决缺 gate 三类错误均被正确透传（有测试）。
- [ ] core 改动仅为 additive `to_json`。

## 不在本里程碑（明确排除）

branch / decision / forage / trace 的 CLI（→ C2）、主循环（→ H7）、实际检索工具脚本。
