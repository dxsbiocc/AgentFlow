# ②a 实现简报：serde_json 迁移试点（hypothesis.rs，建立模式）

Status: Implemented + verified (2026-06-03)
Date: 2026-06-03
Owner(orchestrator): Claude · Executor: Codex
Spec source: 分步全量 serde 迁移第 ② 步（serde_json）的试点 —— 先在最简单自包含模块证明模式，再铺开
Depends on: Y1-serde（已合并 main）

## 验收记录（Claude 独立复验 2026-06-03）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **201**（基线 198，+3）/ cli 61 / schemas 3 全绿；`acceptance-v1.sh` 通过。
- ✅ **to_json byte-identical（Claude 独立对照基线）**：`hypothesis show --json` 输出归一化 id/时间戳后与重构前**逐字节一致**。
- ✅ 改动局限 `hypothesis.rs`（152/150，手写 JSON→serde）+ Cargo（加 serde_json）；本模块重复 JSON 助手清零。
- ✅ 旧 payload 兼容（serde_json::from_str 读旧手写格式，覆盖乱序/空白）；枚举 serde 字符串 == as_str。
- ✅ 领域结构体公开 API 不变。

结论：合并就绪。**serde_json 迁移模式确立**（payload 同名同序结构体 derive + to_json 走 serde + 删本地助手 + byte-identical），可逐模块复制到 argument/branch/handoff/forage/trace_guard/agent/comparison/report/observer/graph_patch。

## 目标

在 `hypothesis.rs` 上把手写 JSON 全换成 **serde_json**：事件 payload 构建/解析、`Hypothesis::to_json`，并**删除本模块重复的 `escape_json`/`json_string_field`/`unescape_json_string` 等助手**。**输出 byte-identical**——这是 ② 全量迁移的「模式样板」，验证无误后逐模块复制。

## 编排者裁决（约束）

1. **输出 byte-identical / 行为零变化**：`Hypothesis::to_json` 与事件 payload 的 JSON **逐字节不变**；现有 hypothesis 相关测试**不改且通过**（它们已断言 to_json/解析，是回归铁证）。持久化的旧 payload 仍能被 `serde_json::from_str` 读出（字段名匹配，顺序无关）。
2. **领域结构体公开定义不变**：`Hypothesis`/`HypothesisRequest`/枚举的公开 API 不变。枚举（`HypothesisStatus`/`Confidence`）加 `#[derive(Serialize, Deserialize)]` + `#[serde(rename_all = "snake_case")]`（或逐变体 rename）使其序列化为与现有 `as_str()` **完全相同**的字符串（如 `proposed`/`under_test`/`low`）。
3. **byte-identical 做法**：定义/复用与现有 JSON 字段**同名同顺序**的（payload）结构体派生 `Serialize/Deserialize`；`to_json` 改为 `serde_json::to_string(self)`（字段声明顺序 = 现有 JSON key 顺序 → 紧凑输出一致）。
4. **删重复助手**：本模块的 `escape_json`/`json_string_field`/`unescape_json_string`/`json_nullable_string_field` 等本地副本删除（若被其它模块共用则保留——但本步只动 hypothesis.rs，确认这些是本模块私有副本）。
5. 依赖加 `serde_json = "1"` 到 `crates/agentflow-core/Cargo.toml`（serde 已在）。仅改 `hypothesis.rs`（+ 若枚举定义在别处则加 derive，但不改其逻辑）。

## 交付物

- `Cargo.toml`：加 `serde_json = "1"`。
- `hypothesis.rs`：
  - `hypothesis_created_payload_json` / `hypothesis_transitioned_payload_json` → 用 payload 结构体 + `serde_json::to_string`。
  - 投影里的 payload 解析（`json_string_field` 等）→ `serde_json::from_str` 进 payload 结构体；解析失败 → `StorageError`。
  - `Hypothesis::to_json` → `serde_json::to_string(self)`（结构体派生 Serialize，字段顺序对齐）。
  - 删本模块重复 JSON 助手。
- 枚举 `HypothesisStatus`/`Confidence` 加 serde derive + rename（不改 as_str/parse/逻辑）。

## 验收标准（Claude 审核逐条核对）

- [ ] **byte-identical 铁证**：现有 hypothesis 测试不改且通过；`Hypothesis::to_json` 与 payload JSON 与 main 上一致（Claude 会建假设并对照 `hypothesis show --json` 输出）。
- [ ] `clippy -D warnings` 无警告；`cargo test` 全绿；`acceptance-v1.sh` 通过。
- [ ] 旧 payload 兼容：手写格式的旧事件能被新 `from_str` 读出（顺序/空白无关）——加一个测试用旧格式 payload 字符串构造并解析。
- [ ] 本模块重复 JSON 助手已删除；仅改 hypothesis.rs（+ 枚举 derive）+ Cargo.toml 加 serde_json。
- [ ] 枚举序列化字符串 == 现有 `as_str()`（如 `under_test`），有断言。

## 不在本里程碑（明确排除）

- 其它模块（argument/branch/handoff/forage/trace_guard/agent/comparison/report/observer/graph_patch）的 serde_json 迁移 —— 待本模式验证后逐批铺开。
- 共享 JSON 助手的统一/抽取（本步只删 hypothesis 私有副本）。
- clap（步骤 ③）。
