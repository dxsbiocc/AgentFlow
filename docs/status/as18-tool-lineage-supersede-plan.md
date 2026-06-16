# AS18 实现简报：工具谱系与扬弃（lineage / supersede，进化引擎收尾）

Status: Assigned to Codex（worktree /tmp/af-as18，分支 feat/as18-lineage，从 main）
Spec source: docs/design/tool-evolution-engine-design.md §4⑤(扬弃/谱系) / §11.3(分级治理)
Depends on: AS15–AS17.2(进化引擎检测+验证门)；Plan A 已证 promotable 在 verified+确认参数下可达

## 背景与范围

进化引擎已能**检测**可泛化候选(AS16)并**验证**(AS17 promotable)。AS18 补**采纳与谱系**——当人审阅一个已验证的泛化后,用一个**人在环命令**把旧特化工具标记为被新通用工具**取代(superseded)**,记录谱系,不删除旧版本(扬弃:保留+否定+提升)。这是 §11.3 的**人工确认治理门**(人显式运行命令 = 采纳),也是最低风险、通用、无 LLM 的"质变收尾"。

**本里程碑只做谱系/取代的记账 + 匹配降权 + 报告展示;不做** cohort 推断进核心 param-filling、不自动改写工具源(那些更深的留给后续)。

## 编排者裁决（实现）

### 1. supersede 谱系（core: storage/tool_registry）

- 新增一个**取代事件/记录**:`tool_superseded`(append-only event),payload 含 `superseded_tool_ref`、`successor_tool_ref`、`reason`、时间。复用既有 event-sourcing 模式(参考 hypothesis/decision 的 append_event)。
- 新增查询:`fn tool_supersession(&self, tool_ref) -> Option<{successor, reason}>`(某工具是否被取代)与/或 `list_supersessions()`。
- **不删除**被取代工具;它仍可 inspect,但带"superseded_by <successor>"信息。
- 通用:无任何具体基因/疾病/工具名常量。

### 2. CLI 命令（agentflow-cli）

`agentflow tools supersede <old_tool_ref> --by <new_tool_ref> [--reason <text>] [--json] [--path <p>]`:
- 校验两个 tool_ref 都已注册;记录取代事件;输出确认。
- 这是**人在环采纳门**:人审阅 AS17 的 promotable 验证后,显式运行它来采纳泛化。
- `agentflow tools list` / `tools inspect` 输出体现 superseded 状态(被取代工具标 `superseded_by <successor>`)。

### 3. 匹配降权（core: tool_select / match_tools）

- `match_tools` 里,被取代的工具**降权或排在后**(让 successor 优先被选)。最简:被取代工具 fit 不升、或在排序里靠后(加一个 `superseded` 负向因素,reason 标 `superseded`)。不要直接从候选里删除(保持可追溯/可解释),只降优先级。

### 4. 报告谱系（core: report.rs，AS13 Methods & Tools 段）

- AS13 的 `## Methods & Tools` 段:被取代工具行追加 `— superseded_by <successor>`;successor 行可注 `(supersedes <old>)`。让工具库的进化在报告里可审计。

### 5. 不变量

- 不碰 `argument.rs`(判决确定性 0 LLM/网络);本里程碑**无 LLM/网络**(纯记账+匹配+渲染)。
- 不改 `DecisionKind`/判决/证据 schema;supersede 是工具注册表的 additive 事件,向后兼容。
- 核心无单次任务/领域常量。无新依赖。
- 既有 AS1–AS17.2 测试保持绿。

## 测试（离线确定性）

- core(tool_registry):supersede 事件记录 + 查询;被取代工具仍可 inspect 且带 successor 信息;list 体现状态。
- core(tool_select):两个同能力工具,old 被 supersede → match_tools 里 successor 优先于 old(old 降权/靠后),old 不被删除。
- core(report):research report 的 Methods & Tools 段显示 `superseded_by`。
- cli:`tools supersede` 命令记录并在 `tools list/inspect` 体现;未注册的 ref 报错。
- core 测试数预期 +4~6。

## 验收

- [ ] fmt/clippy/core/cli/acceptance 全绿;`argument.rs` 仍 0 LLM/网络。
- [ ] supersede 记账 + 查询 + 匹配降权(不删除)+ 报告谱系 + CLI 命令,全部通用、确定性、无 LLM。
- [ ] additive 事件向后兼容;核心无领域常量;无新依赖;既有测试保持绿。

## 不在本里程碑

- 不做 cohort 推断进核心 param-filling、不自动改写工具源、不自动 register 泛化工具版本(adoption 由人显式 `tools supersede`,successor 工具本身的产生是 AS17/人提供)。
- 不引入 LLM/网络;不改判决逻辑。
