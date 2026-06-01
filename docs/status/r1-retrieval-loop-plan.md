# R1 实现简报：检索闭环（真实 PubMed 检索工具 + 摄入证据账本）

Status: Implemented + verified (2026-06-01)
Date: 2026-06-01
Owner(orchestrator): Claude · Executor: Codex
Spec source: H6「检索即注册工具（外置进程）」架构决策的落地
Depends on: H6（forage.rs，已验收）

## 验收记录（Claude 独立复验 2026-06-01）

- ✅ `clippy -D warnings` 无警告；`cargo test` cli **40**（基线 35，+5）/ core 155 / schemas 3 全绿。
- ✅ Rust 零新增依赖（Cargo 零变更）；`pubmed_search.py` 仅 `argparse/json/urllib`。
- ✅ `agent_ops_commands.rs` 仅在 forage 子分发新增 ingest/fetch + import 增补（`{Path,PathBuf}`/`Command`/`SystemTime`）；现有 forage handler 零删改（hunk 全为追加）。
- ✅ 离线冒烟：`forage ingest` fixture 摄入 2 条、非法 access_status 报错（"invalid access_status: bogus"）。
- ✅ **真实联网冒烟**：`forage fetch --query "KRAS G12C resistance" --max 3` 实拉 NCBI 真实论文（如 "Targeting KRAS G12C in NSCLC..."），全闭环 `fetch → forage obs → link → evidence → agent run` 跑通；abstract 级证据落 Provisional（§15 合规在真实数据上成立）。

结论：合并就绪。检索闭环打通——forage 可拉真实文献进证据账本。

## 目标

让 forage 从「只有契约」变成「真能拉到文献」：写一个**真实 PubMed 检索脚本（外置进程，网络只在脚本里）**，并加 CLI 把其输出摄入为 forage 观察。Rust 侧**零新增依赖**。

## 架构原则（不可违反）

- **网络只在 Python 脚本里**（urllib 标准库）。Rust core/CLI **不做 HTTP、不新增任何 crate 依赖**。
- 拆两层：`forage ingest <file>`（解析 hits 文件 → 落 forage 观察，**离线确定性可测**）+ `forage fetch --query`（跑脚本再 ingest，联网便利层）。

## 硬约束

1. Rust 侧不新增依赖；JSONL 手写解析（复用现有 json 私有助手风格）；子进程用 `std::process::Command`。
2. CLI 改动只允许**给 `agent_ops_commands.rs` 的 `forage` 子分发新增 `ingest`/`fetch` 两个子命令**；不得改写现有 forage 子命令 handler 或其他现有 handler/测试。
3. 复用 H6 现有 `record_forage_observation`，不改 forage.rs 既有逻辑。
4. 质量门全绿：`clippy -D warnings` + `cargo test`。**基线 core 155 / cli 35 / schemas 3，不得破坏。**

## 交付物

### 1. `examples/tools/pubmed_search.py`（新建，Python 标准库 only）

- 参数：`--query <q> --max <n> --out <file>`。
- 行为：用 NCBI E-utilities（`esearch` 取 PMID 列表 + `esummary` 取标题），**仅用 `urllib`/`json`/`argparse` 标准库**，把结果写为 **JSONL**（每行一个对象）：
  ```json
  {"external_id":"PMID:39000001","title":"...","access_status":"abstract_available"}
  ```
  - PubMed 是元数据/摘要级 → `access_status` 用 `"abstract_available"`。
  - 网络/解析失败：跳过该条或整体写空文件并非零退出码（不要伪造数据）。
- 顶部加用法注释；不依赖任何第三方包。

### 2. CLI `forage ingest`（离线可测，核心）

在 `agent_ops_commands.rs` 的 `forage` 子分发加：
- `forage ingest <hits-file> [--source <id>] [--json] [--path <p>]`
  - 默认 `--source pubmed`。
  - 逐行解析 JSONL，提取 `external_id` / `title` / `access_status`（用 `AccessStatus::parse`，非法值 → `CliError::InvalidArgument`）。
  - 每条调用 `record_forage_observation(source, external_id, title, access_status)`。
  - 返回摘要：成功摄入 N 条 + 各 forage 观察 id（`--json` 输出 id 数组）。
  - 空行跳过；文件不存在 → 错误透传。

### 3. CLI `forage fetch`（联网便利层）

- `forage fetch --query <q> [--source pubmed] [--script <path>] [--max <n>] [--python <bin>] [--json] [--path <p>]`
  - 默认 `--script examples/tools/pubmed_search.py`、`--max 10`、`--python python3`。
  - 用 `std::process::Command` 跑 `<python> <script> --query <q> --max <n> --out <tmpfile>`（tmpfile 用进程内临时路径）。
  - 脚本非零退出 → `CliError`（含 stderr 摘要）。
  - 成功后复用 ingest 逻辑把 tmpfile 摄入；返回同样的摘要。

## 验收标准（Claude 审核逐条核对）

- [ ] `clippy -D warnings` 无警告；`cargo test` 全绿，cli 较 35 净增。
- [ ] 无新增 Rust 依赖；现有 35 cli 测试未改且通过。
- [ ] `forage ingest` 用**fixture JSONL**（测试内写临时文件）离线测试：正常摄入计数正确、非法 access_status 报错、空行跳过、文件不存在报错。
- [ ] `forage fetch` 参数校验 + 脚本缺失/非零退出错误路径有测试（网络 happy-path 不做单测）。
- [ ] `agent_ops_commands.rs` 仅在 forage 子分发**新增** ingest/fetch 分支；现有 forage 子命令 handler 未改（`git diff` 核对）。
- [ ] `pubmed_search.py` 仅用标准库（grep 确认无第三方 import）。

## 不在本里程碑（明确排除）

把检索工具注册进 tool registry 并走 flow/runtime/observer 全路径（后续，可选）、bioRxiv/其它来源、全文获取、Unpaywall 解析、CLI 之外的自动 forage（H7b 主循环触发）。
