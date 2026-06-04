# PV1 实现简报：参数值约束校验闸门（修审计 🟠 参数推断信任缺口）

Status: Assigned to Codex
Owner: Claude(编排) · Codex(执行)
Spec source: 深度测试审计 🟠「参数推断信任缺口」—— 用户选定「校验闸门优先」
Depends on: L1/L2（infer_replace_params/ParamInferer）、flow validate（validate_step_against_tool）、serde 迁移 —— 均已合并 main

## 背景与缺口
当前 LLM 推断的参数值（如 `gene: THRSP`）经 `agent.rs:infer_replace_params` 原样写进 `step.params`，apply/run 前**无任何值校验**（`ToolParamSpec` 只有 type_name+required，type_name 也未强校验）。若 LLM 幻觉出不存在/拼错的值，下游工具可能返回空结果被误读成真实「无关联」发现——违反控制宪章反自欺 + 用户「never trust external data」规则。本里程碑给参数值加**声明式约束 + 边界校验闸门**。

## 目标
1. `ToolParamSpec` 增可选约束字段，工具在 YAML 声明；
2. 单一校验函数对参数值做 类型 + 约束 校验；
3. 两处复用该函数：apply/approve 边界（非法 fail fast）、自治循环推断填值（非法保留占位、不 run 坏值）；
4. **无约束的现有工具零回归**（含 spec hash 不变）。

## 编排者裁决（约束）
1. **可选约束、默认无**：`ToolParamSpec` 加 `enum_values: Option<Vec<String>>`、`pattern: Option<String>`（regex）。两者皆 None 时该参数无值约束（除类型）。serde raw 结构加 `enum`/`pattern` 字段（`#[serde(default)]`，YAML 键名 `enum`/`pattern`），映射进领域结构体；`ExecutableToolSpec` 透传。
2. **type_name 强化**：值按 type_name 解析校验——`int`→i64 可解析、`float`→f64、`bool`→true/false、`string`→恒过。未知 type_name 维持现状（不收紧，避免回归）。
3. **单一校验函数**（DRY）：`fn validate_param_value(spec: &ToolParamSpec, value: &str) -> Result<(), String>`（放 tool_registry 或合适公共处），按 类型→enum→pattern 顺序校验，返回清晰错误消息。**两处复用**：
   - `validate_step_against_tool`（flow_registry.rs:664）：对每个**已提供且非 `REPLACE_<name>` 占位**的参数值调用；非法 → push `FlowValidationIssue`（flow validate/approve fail fast，绝不静默 run 坏值）。占位符 `REPLACE_<name>` 仍按现有「未填」语义处理，不在此报值约束错误。
   - `infer_replace_params`（agent.rs:624）：推断出替换值后，先 `validate_param_value` 校验；**非法则不写入、保留 `REPLACE_<name>` 占位**（坏猜测不变成可 run 步骤）。可选：记录被拒原因供可见。
4. **regex 库**：加 `regex = "1"` 到 `crates/agentflow-core/Cargo.toml`（用户已认可用成熟库）。pattern 编译失败 → 视为工具 spec 错误（InvalidInput / validation issue），不 panic。
5. **零回归铁证**：无 enum/pattern 的现有工具 **spec hash 必须逐字节不变**（Option 字段为 None 不得改变被 hash 的表示）；现有全部测试不改且通过；acceptance 通过。
6. 演示：给 `examples/tools/tcga_survival_assoc.py` 的工具 YAML（tcga_survival_assoc.tool.yaml）的 `gene` 参数加 `pattern: "^[A-Za-z0-9-]+$"`（合理基因符号格式）——该工具 spec hash **会变（预期）**，其余工具不变。

## 交付物
- `Cargo.toml`：加 regex。
- `tool_registry.rs`：`ToolParamSpec` 加 `enum_values`/`pattern`；raw 反序列化加 `enum`/`pattern`；`validate_param_value`；spec hash 对 None 约束保持原表示。
- `flow_registry.rs`：`validate_step_against_tool` 对非占位参数值调 `validate_param_value`，非法 push issue；`ExecutableToolSpec` 透传约束。
- `agent.rs`：`infer_replace_params` 校验推断值，非法保留占位。
- `examples/tools/tcga_survival_assoc.tool.yaml`：gene 加 pattern。
- 测试：值合法过、enum/pattern/type 非法在 validate 报 issue、推断非法值→保留占位（loop 不 run）、无约束工具行为/hash 不变。

## 验收标准（Claude 逐条复核）
- [ ] clippy -D warnings 干净；cargo test 全绿；acceptance 通过。
- [ ] **无约束工具 spec hash 逐字节不变**（marker=2f8e22fc89c1caf9 等）；tcga hash 变化是预期且记录新值。
- [ ] validate 边界：非法 enum/pattern/type 值产生清晰 FlowValidationIssue；占位符不误报。
- [ ] 循环：推断出非法值 → 保留 `REPLACE_`，步骤不 auto-run（行为测试，stub inferer 返回非法值）。
- [ ] 仅新增 regex 依赖；agentflow-cli/schemas 未改依赖；现有测试不改。

## 不在本里程碑
- 溯源 + 证据诚实封顶（审计 🟠 的 B 面，下一里程碑 PV2）。
- 跨参数/语义校验（如 gene 是否真实存在的在线核验）。
- 文献/数据孤岛（🟡，独立后续）。

## 验收记录（Claude 独立复验 2026-06-04）
- ✅ clippy -D warnings 干净；cargo test core 240(+6)/cli 57+2/schemas 3 全绿；acceptance 通过；fmt 通过。
- ✅ **零回归铁证（亲验）**：无 enum/pattern 的 marker 工具 spec hash 仍 `2f8e22fc89c1caf9`；tcga 加 gene pattern 后 hash 变为 `a3e9aca548cab542`（预期）。
- ✅ **校验闸门端到端实证**：gene="BAD GENE/x" → flow validate 报 `param gene invalid ... must match pattern "^[A-Za-z0-9-]+$"`；gene="THRSP" 无 pattern issue。
- ✅ validate_param_value 顺序 type→enum→pattern；pattern 编译失败返错不 panic；占位符不误报；stub 推断非法值→保留占位、apply/auto-run 不执行。
- ✅ 仅 agentflow-core + tcga example yaml；只加 regex 依赖；cli/schemas 未动。
结论：合并就绪。审计 🟠 参数信任缺口「校验闸门」面闭合——推断/人填参数值在 apply/run 边界按工具声明的 type/enum/pattern 校验，坏值 fail fast、坏猜测不变成 run。剩 PV2（溯源+证据诚实封顶）。
