# S2 实现简报：合成/exploratory 工具证据 grade 封顶

Status: Implemented + verified (2026-06-03)
Date: 2026-06-03
Owner(orchestrator): Claude · Executor: Codex
Spec source: S1 后续 —— 让「合成工具=低信任」从标签变成强制（自动触发合成 S3 的安全前提）
Depends on: S1（synth → exploratory 工具）、H1（link_evidence / 三态判决）—— 均已合并 main

## 验收记录（Claude 独立复验 2026-06-03）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **182**（基线 180，+2）/ cli 53 / schemas 3 全绿。
- ✅ 改动局限 `argument.rs`（`link_evidence` 前 `capped_evidence_grade` + `source_tool_maturity_for_observation` 溯源链）；runtime/observer 未改；Cargo 零变更；无新表/event_type/依赖。
- ✅ 仅 `Observed→Inferred` 封顶（exploratory 来源）；非 exploratory / 无 observation_id / 链断 → 不封顶。
- ✅ **live 双向验证**（脚本 co-located 让 exploratory/verified 工具真实跑出观察）：exploratory 工具观察 link `observed` → 存储 `inferred`（封顶）；verified 工具同样 link → 保持 `observed`。
- ✅ 安全闭环：封顶后仅 exploratory 证据无 `Observed` → `has_obs_support` 假 → 不可能 `Affirmed`（单测覆盖）。

> 验收插曲（记录验证纪律）：首次 live 冒烟「看似封顶失效」，诊断发现是**冒烟 setup 错误**（sed 出的工具脚本未 co-located → 运行失败 → 空观察 → 空 observation_id 本就不封顶），非 S2 缺陷；脚本 co-located 重测后封顶正确。**单测绿不等于真生效，live 验证不可省。**

结论：合并就绪。「合成/exploratory 工具=低信任」从标签变成机制保证，S3 循环自动触发合成的安全前提就位。

## 目标

**强制**：来自 exploratory 成熟度工具（含 `synth` 合成工具）的证据，grade 被封顶到 `Inferred`，**永不为 `Observed`**。效果：规则引擎的 `has_obs_support` 不被满足 → **合成工具的证据无法独立驱动 `affirmed` 判决**。这让 S1 的「低信任」成为机制保证，是 S3「循环自动触发合成」的安全前提。

## 编排者裁决（约束）

1. **改动尽量自包含**：优先在 `argument.rs::link_evidence` 处实现——若 `request.observation_id` 指向的观察其来源步骤所用工具是 exploratory，则把请求 grade 封顶到 `Inferred`（`Observed`→`Inferred`；其余 grade 不变）。溯源链：observation(`flow_id`,`step_id`) → 该 flow 中 step 的 tool_ref → `inspect_tool` 的 maturity。可加最小 flow/tool 查询助手。
2. **默认/向后兼容零变化**：观察无法解析到工具、或工具非 exploratory（wrapped/verified/无 maturity）、或证据无 observation_id → **不封顶**，行为与现状完全一致；现有 180 core 测试不改且通过（首要回归）。
3. 不新增依赖/表/event_type。
4. 质量门全绿：`clippy -D warnings` + `cargo test`。基线 core 180 / cli 53 / schemas 3。

## 交付物（`argument.rs`，必要时加最小 flow/tool 查询助手）

- `link_evidence` 内：解析 `observation_id` → 来源工具 maturity；若 `exploratory` 且请求 grade 为 `Observed` → 实际存储 `Inferred`（封顶）。封顶发生时在返回/记录中可体现（如 note 追加或日志），但**核心是存储的 grade 被改为 Inferred**。
- 一个内部助手，例如 `fn source_tool_maturity_for_observation(&self, observation_id) -> Result<Option<ToolMaturity>, StorageError>`：observation → (flow_id, step_id) → flow 中该 step 的 tool_ref → `inspect_tool` → maturity；任何一环缺失返回 `None`（不封顶）。
- 仅封顶 `Observed`→`Inferred`。`Inferred`/`LiteratureSupported`/`Hypothesis`/`Unsupported` 不变（它们本就 ≤ Inferred 或非项目观测）。

## 验收标准（Claude 审核逐条核对）

- [ ] **回归**：无 observation_id / 非 exploratory 来源时行为零变化；现有 180 core / 53 cli 测试不改且通过（首要）。
- [ ] `clippy -D warnings` 无警告；`cargo test` 净增、全绿。
- [ ] 无新依赖/表/event_type。
- [ ] 封顶测试：①注册一个 **exploratory** 工具 → 跑出观察 → `link_evidence(observation_id, grade=Observed)` → 存储 grade 实为 `Inferred`（断言）；②注册一个 **wrapped/verified** 工具 → 同样 link Observed → 仍为 `Observed`（不封顶）；③无 observation_id 的 Observed 证据 → 不封顶。
- [ ] **安全闭环测试**：仅有 exploratory 来源证据时，`render_verdict` **不可能** Affirmed（因封顶后无 Observed → `has_obs_support` 假）；换成 verified 来源同等证据则可预览 Affirmed。
- [ ] grep 确认改动局限 argument.rs（+ 必要的最小 flow/tool 查询助手）；不改 runtime/observer 的现有行为。

## 不在本里程碑（明确排除）

- 循环自动触发合成（agent run 遇 no-tool-match → 自动 synth）—— S3（本封顶就位后才安全）。
- LLM 接进循环引擎（ArgumentEngine/BranchSelector）—— 独立后续。
- 手动 `observe`（非工具来源）/ forage 证据的封顶 —— 本步只覆盖「工具产出的观察」这条主链。
- 真沙箱隔离。
