# 简报：artifact reference 模式的不可信导入加固(#58, LOW)

Status: Assigned to Codex（worktree /tmp/af-h58，branch fix/artifact-reference-safety，从 main 起）
来源：发布前安全审计低 severity follow-up。

## 漏洞(LOW)

`crates/agentflow-core/src/storage/artifact_registry.rs:~106`、`crates/agentflow-cli/src/lib.rs:~1064`：默认导入模式是 **reference**。若攻击者诱导导入 `/etc/passwd` 或一个指向敏感目标的 symlink，后续 flow 可引用该工件并把外部路径传给工具。list/inspect 也暴露绝对路径。这不是 `.agentflow` 内的拷贝路径穿越 bug，但对不可信导入很危险。

## 修复要求(最小、最少破坏现有行为)

**不要**粗暴翻转全局默认(reference→copy 会改变现有 demo/测试行为)。改为按"项目根内引用放行、项目根外引用需显式 opt-in"加固：

1. **限制 reference 到项目根内**:默认情况下,reference 模式只允许引用解析(canonicalize,解 symlink 后)仍落在**项目根目录树内**的路径;指向项目根外的 reference 默认**拒绝**并给明确错误,提示用 `--allow-external-reference` 或 copy 模式。
   - 注意 canonicalize 要解 symlink:一个项目根内的 symlink 指向 `/etc/passwd` 也必须被外部判定拦下。
2. **新增 CLI flag `--allow-external-reference`**(lib.rs import 命令):显式放行项目根外的 reference,供合法跨目录场景。默认关闭。copy 模式不受此限制(拷贝进 `.agentflow` 本就安全)。
3. **绝对路径展示**:在 list/inspect 的 JSON/人读输出里,对引用的外部绝对路径做**相对化或脱敏**(如显示相对项目根的路径,或对根外路径标注;不要在常规 JSON 里直接吐完整宿主绝对路径)。保持可定位但不过度泄露宿主布局。可保守实现:对项目根内路径显示相对路径;根外(已 opt-in)路径保留但标注。

实现要保守:现有 `examples/data/*` 在项目内导入的流程(README quick-start、acceptance)必须继续工作(它们引用的是 repo 内路径,canonicalize 后在项目根/仓库内——确认 acceptance 用的 `--path "$AF_DEMO"` 项目根与被导入文件的关系,必要时把"项目根内"判定放宽到"导入源与项目根的合理关系",核心是默认拦截 `/etc/passwd` 这类宿主敏感路径,而不是误伤 demo)。若 demo 导入的是 repo 内、项目目录外的路径,确保 `--allow-external-reference` 或合理默认不破坏 acceptance——优先保证 acceptance 绿。

## 不变量(硬约束)

- `git diff crates/agentflow-core/src/argument.rs` 为空;不碰判决逻辑。
- 仅改 `artifact_registry.rs` + `lib.rs`(+ 各自测试);不碰 runtime/synth(那是 #57/#59,另一个 worktree)。
- 不引入新依赖。
- 现有 acceptance + 现有测试全绿。

## 测试(离线)

- 单测:reference 一个项目根外路径(如 tempdir 外的文件)默认被拒;加 `--allow-external-reference`/对应 request 字段后放行;reference 项目根内文件正常;**项目根内 symlink 指向根外目标**被拒(canonicalize 解 symlink)。
- list/inspect 输出对外部绝对路径脱敏/相对化的断言。
- `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test -p agentflow-core` + `cargo test -p agentflow-cli`(相关)、`bash scripts/acceptance-v1.sh`。**不要** `cargo test --workspace`。

不要 commit。报告:改了哪些行、新增测试、确认 argument.rs 未动、acceptance 绿、demo 导入未误伤。
