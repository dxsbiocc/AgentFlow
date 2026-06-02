# F1 实现简报：auto-forage 闭环（opt-in，默认关）

Status: Implemented + verified (2026-06-02)
Date: 2026-06-02
Owner(orchestrator): Claude · Executor: Codex
Spec source: 待完善清单 #1（高优先）—— 让循环自动为证据不足的假设拉文献
Depends on: R1（forage fetch/ingest）、H6（link_forage_evidence）、H7a/T2（agent run）—— 均已合并 main

## 验收记录（Claude 独立复验 2026-06-02）

> 注：首次分派在会话切换时被 kill（未执行），已重跑成功。

- ✅ `clippy -D warnings` 无警告；`cargo test` cli **48**（基线 44，+4）/ core **172（不变）** / schemas 3 全绿。
- ✅ 改动严格局限 `agent_ops_commands.rs` + usage；**core 零改动**；无新依赖/表/event_type。
- ✅ 默认关零行为变化：无 `--auto-forage` 时 `agent run` 直接返回原 CycleReport。
- ✅ `should_auto_forage` 仅对 `None | InconclusiveProvisional` 返回 true（强判决不 forage）；链入 stance=`Neutral`、note=`auto-forage`；单假设脚本失败记入 skipped 并继续。
- ✅ 离线 fixture 测试覆盖 provisional / 强判决跳过 / 单失败继续。
- ✅ **端到端冒烟**（fixture 脚本注入）：无 flag → 0 forage；`--auto-forage` → foraged 1 假设、linked 2 观察、证据以 `[hypothesis/neutral] auto-forage` 入账。

结论：合并就绪。循环现可自主为证据不足的假设拉文献入账（Neutral，不自动结案，符合 §15）。stance 自动判定（for/against）需全文/NLP，留后续。

## 目标

给 `agent run` 加 `--auto-forage`（**opt-in 默认关**）：跑循环前，对**证据不足的假设**自动用 PubMed 脚本拉文献 → 摄入 forage 观察 → 链入证据账本（Neutral）→ 再跑常规循环。让循环真正自我补充证据。

## 编排者设计裁决（不可违反）

1. **网络/子进程留在 CLI 层**：全部改动在 `crates/agentflow-cli/src/agent_ops_commands.rs`（复用 R1 的 `forage fetch` 机制）。**core 的 `run_cycle` / forage.rs / 其它已验收模块逻辑不改**，只调用其公开 API（`list_hypotheses` / `latest_verdict_for` / `link_forage_evidence`）。
2. **默认关 = 零行为变化**：不传 `--auto-forage`，`agent run` 行为与现状逐字节相同；现有 44 cli 测试**不改且通过**。
3. **诚实的 stance**：自动拉到的文献链为 **`Stance::Neutral`**（"找到相关文献"，未判 for/against）；stance 判定需全文/NLP，不在本步。说明：Neutral + abstract grade（权重0）**不会**移动判决——这符合 §15（摘要不能定论），auto-forage 的价值是**自主把相关文献沉淀进证据账本**，不是自动结案。
4. **只 forage 证据不足者**：仅对 `latest_verdict_for` 为 **Provisional 或 None** 的假设 forage；已是强判决/已交接的不动。
5. 不新增依赖；不新增 event_type/表。

## 交付物（`agent_ops_commands.rs`）

### `agent run` 新增 flag
- `--auto-forage`（bool，默认 false）
- `--forage-max <n>`（每个假设最多拉取，默认 5）
- 复用现有 `--forage-script <path>`（默认 `examples/tools/pubmed_search.py`）与 `--python <bin>`（便于测试注入 fixture 脚本）

### 重构（additive 提取，行为不变）
从 `forage_fetch_command` 提取可复用助手：
```rust
fn fetch_and_ingest(
    store: &ProjectStore, python: &str, script: &Path,
    query: &str, max: u32, source: &str,
) -> Result<Vec<ForageObservation>, CliError>;  // 跑脚本→ingest_forage_hits
```
`forage_fetch_command` 改为调用它（保持原行为与原测试通过）。

### auto-forage pass
```rust
struct AutoForageSummary { hypotheses_foraged: usize, observations_linked: usize, skipped: Vec<String> }

fn auto_forage_pass(store: &ProjectStore, /* python, script, max */ ...) -> Result<AutoForageSummary, CliError>;
// 对每个 Provisional/None 的假设：
//   query = hypothesis.statement
//   obs = fetch_and_ingest(...);  对每个 obs: store.link_forage_evidence(hyp_id, obs.id, Stance::Neutral, "auto-forage")
//   单个假设脚本失败 → 记入 skipped，继续（不中断整轮）
```

### `agent_run_command` 接线
- 当 `--auto-forage`：先 `auto_forage_pass`，再照常 `run_cycle_with_apply_config`。
- 人类输出在 CycleReport 前**前置** Auto-forage 段（foraged M 假设、linked N 观察、skipped 列表）；`--json` 增加 `auto_forage` 字段（additive，不破坏现有 json 消费）。
- 不传 `--auto-forage` 时完全不执行上述路径。

## 验收标准（Claude 审核逐条核对）

- [ ] **回归**：不传 `--auto-forage` 时 `agent run` 行为零变化；现有 44 cli 测试未改且通过（首要）。
- [ ] `clippy -D warnings` 无警告；`cargo test` 净增，全绿。
- [ ] 无新依赖/表/event_type；core 模块逻辑未改（仅 CLI 改动）。
- [ ] **离线**测试（用 fixture 脚本经 `--forage-script` 注入，输出固定 JSONL，不联网）：①一个 Provisional 假设经 auto-forage 被链入 Neutral 证据；②已强判决/已交接的假设不被 forage；③单假设脚本失败被跳过且不中断。
- [ ] `--auto-forage` flag/`--forage-max` 解析测试；链入的证据 stance 为 Neutral。
- [ ] grep 确认改动仅 `agent_ops_commands.rs`（+ 若必要的 usage 行）；core 未改。

## 不在本里程碑（明确排除）

stance 自动判定（for/against，需全文/NLP）、把 auto-forage 放进 core run_cycle、全文获取、查询语句的智能精炼（当前直接用 statement）、把 `--auto-forage` 设为默认。
