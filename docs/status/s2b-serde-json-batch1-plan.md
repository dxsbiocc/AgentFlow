# ②b 实现简报：serde_json 铺开第一批（forage / trace_guard / handoff）

Status: Implemented + verified (2026-06-03)
Date: 2026-06-03
Owner(orchestrator): Claude · Executor: Codex
Spec source: serde 迁移 ② 步，按 ②a（hypothesis.rs）已验证样板铺开
Depends on: ②a（serde_json 模式确立，已合并 main）

## 验收记录（Claude 独立复验 2026-06-03）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **209**（基线 201，+8）/ cli 61 / schemas 3 全绿；`acceptance-v1.sh` 通过。
- ✅ 改动仅 `forage.rs`/`trace_guard.rs`/`handoff.rs`（净删代码：handoff 240/365、trace_guard 169/212、forage 137/130）；三模块本地 JSON 助手清零（grep 确认）。
- ✅ byte-identical：现有测试不改且通过（已断言 JSON）+ 新增 exact-byte to_json/payload 测试固化；旧手写 payload 兼容测试（空白/乱序）；枚举序列化 == as_str 断言。
- ✅ 无新依赖；领域结构体公开 API / 枚举逻辑未改。

结论：合并就绪。serde_json 样板成功铺开第一批。剩余 argument/branch/agent/comparison/report/observer/graph_patch 下一批。

## 目标

把 `forage.rs` / `trace_guard.rs` / `handoff.rs` 三个模块的手写 JSON 全换 **serde_json**，删各自重复的 `escape_json`/`json_string_field`/`unescape_json_string` 等本地助手。**输出 byte-identical、现有测试不改且通过**。serde/serde_json 已是依赖。

## 样板（来自 ②a，已验证）

对每个模块，逐一：
1. 事件 payload 构建（`*_payload_json`）→ 同名同顺序的 payload 结构体派生 `Serialize` + `serde_json::to_string`。
2. 投影里的 payload 解析（`json_string_field` 等）→ payload 结构体派生 `Deserialize` + `serde_json::from_str`；旧手写格式仍可读（字段名匹配、顺序/空白无关）。
3. 公开记录类型的 `to_json` → `serde_json::to_string(self)`，**结构体字段声明顺序 = 现有 JSON key 顺序**（紧凑输出 byte-identical）。
4. 相关枚举加 `#[derive(Serialize, Deserialize)]` + serde rename，使序列化字符串 == 现有 `as_str()`。
5. 删本模块重复 JSON 助手。

## 三模块的类型/枚举（需 serde derive）

- **forage.rs**：`ForageObservation`（to_json）+ payload；枚举 `AccessStatus`（七态，rename == as_str：`metadata_only`/`abstract_available`/…）、`ForageAction`。
- **trace_guard.rs**：`Checkpoint`/`DriftReport`/`RevertRecord` 的 to_json + payload。
- **handoff.rs**：`DecisionPoint`（含 `Vec<HandoffOption>`、`Option<Resolution>`、`status`）的 to_json + payload；枚举 `Cost`/`Risk`/`DecisionKind`/`DecisionStatus`（rename == as_str，如 `deepen_or_stop`/`tool_gap`/`stance_assessment`/`pending`）。`HandoffOption`/`Resolution` 也 derive。

## 编排者裁决（约束）

1. **byte-identical / 现有测试不改且通过**（首要回归）：三模块的 to_json 与 payload JSON 逐字节不变；各模块现有测试不改；旧手写 payload 仍能 from_str 读出。
2. 领域结构体公开 API 不变；枚举 `as_str`/`parse`/逻辑不变（只加 derive+rename）。
3. 仅改这三个 `.rs`（+ 若枚举/结构体定义在别处则加 derive，不改逻辑）。不动其它模块。不新增依赖（serde_json 已在）。
4. 删除三模块重复的本地 JSON 助手；若某助手被本批外模块共用则保留（确认）。

## 验收标准（Claude 审核逐条核对）

- [ ] `clippy -D warnings` 无警告；`cargo test` 全绿；`acceptance-v1.sh` 通过。
- [ ] 三模块现有测试不改且通过；各加一个旧格式 payload 兼容测试。
- [ ] byte-identical：各模块加 exact-byte to_json 测试（固化旧输出）；枚举序列化 == as_str 有断言。
- [ ] 三模块本地重复 JSON 助手清零（grep 确认）。
- [ ] 改动仅 forage.rs/trace_guard.rs/handoff.rs（+ 枚举 derive）；其它模块未动。

## 不在本里程碑（明确排除）

- 其余模块 argument/branch/agent/comparison/report/observer/graph_patch 的迁移（下一批）。
- 跨模块共享 JSON 助手的统一抽取（待全部模块迁完后，若仍有残留再统一清）。
- clap（步骤 ③）。
