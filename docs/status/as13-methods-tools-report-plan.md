# AS13 实现简报：研究报告增加 Methods & Tools 复现段（Robin 原则 B，窄、通用、纯 DB）

Status: Assigned to Codex（新分支 feat/methods-code-report，从 main 起，main 已含 AS7–AS12）
Owner: Claude(编排) · Codex(执行)
Spec source: 对照 Robin（Nature s41586-026-10652-y）Finch "分析即可复现代码" 原则，编排者核查后的安全窄化版本
Depends on: AS1–AS12（已并入 main）

## 背景与范围裁决

Robin 的 Finch 原则 = 分析过程透明可复现。AgentFlow 的"分析"载体是工具（含自主合成工具），它们已被注册、runtime-gate 验证、产出 observation/evidence，但**研究报告没有把"用了哪些工具、它们是什么、怎么复现"呈现出来**。本里程碑补这一段。

**关键安全/范围裁决（必须遵守）**：合成工具的 Python 源码是**磁盘上的单次任务工件**（`.agentflow/local_tools/<name>.py`），工具 spec 的 `runtime_command` 里带其路径。**不把任意磁盘文件内容内联进报告**——原因：(1) 报告可能被分享，内联任意路径文件是信息泄露面（用户可注册指向任意路径的工具）；(2) 体积不可控；(3) 让 core 报告生成去读 spec 里嵌的任意文件路径是耦合/安全异味。因此本段**只渲染 DB 内已有的工具元数据 + 复现指针（脚本路径）**，让人据此找到并复现，而不内联代码本身。符合"不把单次任务代码当核心/不让核心读任意工件"的原则。

## 编排者裁决（约束）

### 在研究报告新增 "## Methods & Tools" 段

`crates/agentflow-core/src/report.rs::generate_research_report_markdown`（约 50）：在现有段落之后（建议放在 "## Research notes" 之前或之后，顺序自定但稳定）新增一段。

内容（**全部来自 DB，纯渲染**）：
- 段标题 `## Methods & Tools ({N})`，N = `self.list_tools()?` 数量。
- 空时一行 `- No tools registered.`。
- 每个工具（按 tool_ref 稳定排序）渲染：
  - `- \`{tool_ref}\` [{maturity}]: {description}`（description 从 `inspect_tool(tool_ref)?.spec_json` 经 `stored_tool_spec_from_json` 取得；若取不到则 `(no description)`，tolerant）。
  - 子行 `  - runtime: {runtime_command join ' '}`（同样来自 stored spec）。
  - **provenance 标记**（通用判定，不写具体工具名常量）：
    - `namespace == "synth"` → `  - provenance: 自主合成工具(runtime-gate 已验证)；源码见 runtime 路径`（路径即 runtime_command 里的脚本参数，已经在上面 runtime 行可见，无需再读文件）。
    - 其余 → `  - provenance: 预置/外部工具`。
- 不读任何磁盘文件；不内联脚本内容；只用 `list_tools` + `inspect_tool`（已有 API）。

### 不变量与约束

- 纯渲染既有 DB 数据；report.rs 已是 store-only，本段不引入文件系统读取。
- 不改证据/事件 payload、tool schema、`DecisionKind`、allowlist、AS7–AS12 逻辑。
- 不引入新依赖；core 仍 0 LLM/网络；**不写入任何具体工具/基因/PMID 常量**到核心（引擎通用列出"当前项目里有什么工具"）。
- 确定性：给定 store 状态，输出稳定（tool_ref 排序）。

## 测试

- `report.rs` 新增单测：
  - 注册一个工具（如现有测试里的 marker 工具 helper）→ research report 含 `## Methods & Tools (1)`、含 `\`marker/...\` [..]: <desc>`、含 `runtime:` 行、provenance 行。
  - 空项目 → 含 `## Methods & Tools (0)` 与 `- No tools registered.`。
  - （可选）注册一个 `synth/` namespace 工具 → provenance 行含 "自主合成工具"。
- 既有 research report 测试（断言段落集合/精确字符串）按新增段更新。
- core 测试数预期 +2~3。

## 验收标准（Claude 复核）

- [ ] fmt / clippy / core / cli / `scripts/acceptance-v1.sh` 全绿；`argument.rs` 仍 0 处 LLM/网络。
- [ ] research report 含 "## Methods & Tools" 段，列出每个工具的 ref/maturity/description/runtime/provenance。
- [ ] synth 工具被标 "自主合成工具(runtime-gate 已验证)"，复现指针(runtime 路径)可见。
- [ ] 不读任意磁盘文件；无新依赖；核心无单次任务常量；core 测试数不减少。

## 不在本里程碑

- 不内联合成工具的 Python 源码（安全/体积/耦合考虑；用 runtime 路径作复现指针）。如确需"报告内可见代码"，另议一个受限方案（只读 `.agentflow/local_tools/` 下、限定前缀、限长、tolerant）。
- 不做 observation→source-tool 的逐条溯源映射（"哪个工具产出了哪条发现"）——可作未来精化。
- 不做原则 C（机制子假设 spawn）/原则 D（候选排序）。
- 不改 `examples/tools/*`（example 工件，不动）。
