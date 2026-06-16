# 简报：promotable → 自动注册泛化版本（候选），人工保留 supersede 采纳门（AS20）

Status: Assigned to Codex（worktree /tmp/af-as20，branch feat/as20-promote-loop，从 main e96091c 起）
Spec source: AS18 PR(#52) 显式 deferred 项之一 —— "auto-registering the generalized version"。闭合进化引擎：检测(AS16)→验证(AS17 promotable)→**注册泛化候选(AS20)**→人工 supersede 采纳(AS18)。

## 治理红线（先读，决定整个设计）

RFC §11 graduated governance + AS18 是**人在环采纳门**：`tools supersede` 必须由人执行。所以：

- **AS20 只自动"注册泛化候选"，绝不自动 supersede / 绝不自动把旧工具下线。**
- 注册的泛化版本 maturity = **exploratory**（候选，未验证）→ 即便被选用，其证据也会被 grade-cap 到 inferred（诚实）。
- 闭环的最后一步（adoption / supersede）仍是人跑 `agentflow tools supersede <old> --by <new>`。AS20 负责把这条命令**算好并显式建议**给人，不替人按下。

注册 ≠ 采纳。这一条是本任务的灵魂，违反即不收。

## 背景（当前事实）

- AS17 泛化验证门在 CLI（`crates/agentflow-cli/src/agent_ops_commands.rs`）：`validate_generalization_*` 产出 `GeneralizationValidation`，verdict ∈ {Promotable, Rejected, Skipped}（枚举 ~1159 行；Promotable 在 ~1330-1346 行设定）。AS17 **从不改工具库**（read-only）—— 保持不变。
- AS18 已加：`store.supersede_tool(old, new, reason)`（append-only 事件，`crates/agentflow-core/src/storage/tool_registry.rs:987`）；CLI `agentflow tools supersede`。`register_tool`（tool_registry.rs:814）。
- 被验证的候选用的是现有工具 + 一个 cohort 参数（验证门已用 `--cohort` 跨 cohort 成功跑过），即**该工具其实已能参数化 cohort**。泛化"版本"主要是：把 cohort 从隐含/特化提升为**显式声明的 param**，并以**新 tool_ref**（如 `<ns>/<name>_general`）登记为候选。**不需要 LLM 重写 python**（运行时脚本同一个）；这让 AS20 可**确定性**生成，CLI 侧完成。

## 编排者裁决（实现）

### 1. 从 promotable 候选确定性派生"泛化 ToolSpec"（CLI，新函数）

输入：promotable 的 `GeneralizationValidation` + 原工具的 `ExecutableToolSpec`/`ToolSpec`。产出一个新的 `ToolSpec`：
- namespace 同原；name = `<原name>_general`（或在简报作者判断下更合适的确定性后缀，但**不得**含具体 study/疾病字样）；version 重置（如 `0.1.0`）。
- 把 cohort 提升为**显式 param**（type string，required）。runtime/inputs/outputs/observer/min_rows 与原工具一致（同一脚本）。
- maturity = **exploratory**。description 标注它是 `<原 tool_ref>` 的 cohort-参数化泛化候选。
- 不引入任何具体 study id / 疾病 / 基因常量。

### 2. 自动注册候选（CLI，仅在 promotable 时）

- 在产出 promotable 后，调用 `store.register_tool(generalized_spec)` 注册该候选（幂等：若同 tool_ref 已存在且 spec_hash 相同则跳过；用 `ToolRegistration.replaced_existing_version` 判定，勿重复刷事件）。
- 持久化一个 append-only 事件（如 `generalization_candidate_registered`），payload 含 {source_tool_ref, generalized_tool_ref, cohort_param, validation 摘要}，便于审计与报告。
- **不调用 supersede。**

### 3. 显式建议人工采纳（CLI 输出 + 报告）

- 在 agent run 的人读/JSON 输出里，promotable 那行后追加一条**建议**：`已注册泛化候选 <generalized_tool_ref>（exploratory）。如经审阅认可，运行: agentflow tools supersede <source_tool_ref> --by <generalized_tool_ref> --reason "<validation 摘要>" 完成采纳。`
- AS13 Methods/报告（`crates/agentflow-core/src/report.rs` 若涉及）只读地体现该候选的存在与谱系；report 改动须保持纯展示，**不**触发注册。

### 4. 边界

- 仅改 `crates/agentflow-cli`（和必要时 `report.rs` 的纯展示）。`git diff crates/agentflow-core/src/argument.rs` 为空；不碰 DecisionKind / 判决逻辑。
- 注册的是**运行时生成的工具候选**（进 registry），**不是**把单次任务的 python 当核心代码写进仓库源码树。不新增 examples/ 下的工件。

## 不变量（硬约束）

- promotable→**注册候选**（exploratory），**绝不自动 supersede**；人工命令被算好并建议。
- 候选 maturity=exploratory（证据可被 cap，诚实）。
- 无具体 study/疾病/基因常量；泛化名/描述不含特化字样。
- 幂等：重复 run 不重复注册/不重复刷事件。
- argument.rs 空 diff；无新依赖；core seam（若需新增）Noop-default。

## 测试（离线，合成，无网络/LLM）

- 单测：给一个**合成** promotable validation + 合成原 ToolSpec → 派生的泛化 spec：name 带 `_general`、含显式 cohort param、maturity=exploratory、无特化字样；register 后 `inspect_tool` 可见；二次调用幂等（不重复事件）。
- 单测：promotable 输出里含 `tools supersede ... --by ..._general` 建议字符串；**断言未发生 supersede**（`list_supersessions` 为空 / 旧工具未被标记 superseded）。
- Rejected/Skipped 时**不**注册、不建议。
- `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`bash scripts/acceptance-v1.sh` 全绿；core（除 report.rs 纯展示外）未改。

## 不在本里程碑

- 不自动 supersede（人工门，AS18 已提供）。
- 不做 cohort 推断进核心 param-filling（AS19，另一个 worktree）。
- 不用 LLM 重写脚本（同一运行时脚本，确定性派生 spec 即可）。
