# Y1（serde 版）实现简报：serde_yaml 替换手写 YAML 解析器

Status: Implemented + verified (2026-06-03)
Date: 2026-06-03
Owner(orchestrator): Claude · Executor: Codex
Spec source: 用户决策「用第三方库，不重复造轮子」+ 分步全量 serde 迁移第 ① 步（顺带修审计 🟠#2 YAML 脆弱）
Depends on: tool_registry / flow_registry 的 from_simple_yaml —— 已合并 main
Supersedes: 早先的 yaml-rust2 值解析方案（已废弃，那是没放下「少依赖」错误思路的半吊子）

## 验收记录（Claude 独立复验 2026-06-03）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **198**（基线 193，+5）/ cli 61 / schemas 3 全绿；`scripts/acceptance-v1.sh` 通过。
- ✅ **行为铁证（Claude 独立复验）**：`marker_survival_scan` spec hash `2f8e22fc89c1caf9`、`tcga_survival_assoc` `83405af4395dc680`——与重构前**逐字节一致**，证明现有 YAML 解析成完全相同的结构体。
- ✅ 依赖只加 `serde`(derive) + `serde_yaml_ng`；领域结构体公开定义/字段/调用点/签名未改（serde 走内部 raw struct + 映射 + 复用现有校验）。
- ✅ **内联语法 live 验证**：`params: {x: {type: string, required: true}}`、`command: [/bin/sh, seed.sh]`、整步内联 `- {id: seed, ...}` 注册/批准成功（修掉审计 🟠#2 + 编排者深度测试踩 4 次的坑）。

结论：合并就绪。serde 迁移第①步——YAML 解析器交给 serde_yaml_ng，行为零变化、内联语法可用。后续 ② serde_json 替换手写 to_json/payload、③ clap 重构 CLI。

## 背景

「core 零依赖」是早先错误地自我强加的，已由用户纠正。本步用 **serde + serde_yaml_ng** 正经替换两个手写 YAML 解析器（`ToolSpec::from_simple_yaml`、`FlowDraft::from_simple_yaml`），消除手写分词器、支持标准 YAML（内联 `{}`/`[]`、引号、注释）、错误带定位。

## 编排者裁决（约束）

1. **领域结构体不变**：`ToolSpec`/`ToolPortSpec`/`ToolParamSpec`/`ToolRuntimeSpec`/`FlowDraft`/`FlowStepDraft` 的**公开定义、字段、所有调用点、StorageError 错误类型不变**。serde 反序列化走**内部 raw 结构体**（`#[derive(Deserialize)]`），再映射进领域结构体并复用现有校验/默认逻辑。
2. **行为铁证：spec hash 不变**。所有现有 `examples/tools/*.tool.yaml`、`examples/flows/*.flow.yaml` 与测试 fixture **解析出完全相同的结构体**——以 `tools register` 打印的 **spec hash 重构前后逐字节一致**为验收铁证；现有全部测试不改且通过；`scripts/acceptance-v1.sh` 通过。
3. **新增**：标准 YAML 内联语法被接受（加新测试断言内联与 block 等价）。
4. 依赖加 `serde = { version = "1", features = ["derive"] }` 与 `serde_yaml_ng`（维护版）到 `crates/agentflow-core/Cargo.toml`。
5. 解析错误映射 `StorageError::InvalidInput`，消息含 serde 的行/列/原因。

## 交付物

### raw 反序列化结构体（内部，`#[derive(Deserialize)]`）
镜像 YAML 形状，处理键名/默认：
- tool：字段 `schema_version/namespace/name/version/maturity/description/validator_profile/inputs/params/outputs/runtime`。
  - 端口 raw：`type`（`#[serde(rename = "type")]` → 映射到 `type_name`）、`required`（`#[serde(default)]`，保留现有默认语义）、`observer/profile/min_rows/sample_id_column`（Option）、`required_columns`（`#[serde(default)]` Vec，支持标量或列表，按现有语义）。
  - param raw：`type` → type_name、`required`（default）。
  - runtime raw：`backend/command/timeout_seconds/env_name/env_prefix/env_file/runner`。
  - maturity：raw 为 String，用现有 `ToolMaturity::parse` 映射（非法值 → InvalidInput）。
- flow：`schema_version/id/name/steps[]`；step：`id/tool/type?/reason/needs/inputs/params/outputs`，按现有语义。

### 映射 + 校验
- `from_simple_yaml`：`serde_yaml_ng::from_str::<Raw>(source_text)` → 映射进领域结构体；`ToolSpec.source_text = source_text.to_string()`；复用现有所有校验与默认（必填字段、`validator_profile` 默认展开、min_rows 等）。
- 保持函数签名 `(source_text: &str) -> Result<Self, StorageError>` 与所有调用点不变。

## 验收标准（Claude 审核逐条核对）

- [ ] **回归铁证**：现有全部测试不改且通过；逐个 `examples/tools/*.tool.yaml`、`examples/flows/*.flow.yaml` 的 **spec hash 与 main 上重构前一致**（Codex 应在实现中对照确认，Claude 复验）；`scripts/acceptance-v1.sh` 通过。
- [ ] `clippy -D warnings` 无警告；`cargo test` 净增、全绿。
- [ ] **内联语法测试**：`params: {gene: TP53}`、`command: [/bin/sh, x.sh]`、`needs: [a, b]`、`inputs: {expr: art_1}` 等内联形式解析成功且与 block 等价。
- [ ] 解析/校验错误返回 `StorageError::InvalidInput` 含定位。
- [ ] 依赖仅新增 `serde` + `serde_yaml_ng`；未动 agentflow-cli/schemas 依赖；领域结构体公开定义未改。

## 不在本里程碑（后续步）

- ② serde_json + derive 替换 core 各模块手写 `to_json` / payload 解析（下一步）。
- ③ clap 重构 CLI 参数解析。
- 本步只做 YAML 两解析器。
