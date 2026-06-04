# PV2 实现简报：参数溯源 + 证据诚实封顶（修审计 🟠 信任缺口 B 面）

Status: Assigned to Codex
Owner: Claude(编排) · Codex(执行)
Spec source: 深度测试审计 🟠「参数推断信任缺口」B 面 —— 让残余信任诚实可见 + 机制封顶
Depends on: PV1（校验闸门）、S2（grade 封顶缝 source_tool_maturity_for_observation）、L4（StanceAssessment digest）、L2（infer_replace_params/ParamInferer）—— 均已合并 main

## 背景
PV1 校验了推断参数值的格式，但一个**格式合法却语义错误**的 LLM 推断值（如把不相关基因猜成 THRSP）仍可能驱动出一个"看起来真实"的观测。PV2 让这种残余信任**诚实可见 + 机制封顶**：记录哪些参数是 LLM 推断的（溯源），并让依赖未确认推断参数的证据**不能冒充 Observed**（复用 S2 封顶机制），且在交接 digest 里可见告警。

## 目标
1. **溯源**：循环用 LLM 推断填充参数时，持久化记录（flow_id, step_id, 推断的参数名/值, hypothesis_id）。
2. **封顶**：observation 的来源步骤若用了 LLM 推断参数 → 该 observation 的证据 grade 封顶 Observed→Inferred（与 exploratory 工具同机制）。
3. **可见**：L4 raise StanceAssessment 时，若来源步骤用了推断参数，digest 追加可见告警列出这些参数。

## 编排者裁决（约束）
1. **infer_replace_params 返回推断参数名**：签名改为返回 `Vec<String>`（本次推断成功填充的参数名）；现有逻辑/校验（PV1）不变。
2. **溯源事件（事件溯源惯用法）**：新增 event_type `agent.params_inferred`，payload `{flow_id, step_id, hypothesis_id, params: [{name, value}]}`。**emit 点**：循环把 drafted_step 经 graph_patch 应用成功后（agent.rs apply 路径，约 498-499），若该步有推断参数则 append 一条。新增投影/查询助手 `inferred_params_for_step(flow_id, step_id) -> Result<Vec<(String,String)>, StorageError>`（无记录返回空）。
3. **封顶扩展（复用 S2 缝）**：`argument.rs::capped_evidence_grade` 增一路：解析 observation → (flow_id, step_id)，若 `inferred_params_for_step` 非空 → Observed 封顶 Inferred。与现有 exploratory 封顶**并存**（任一命中即封顶）；封顶仅 Observed→Inferred，其余 grade 不变。
4. **L4 digest 可见告警**：`agent.rs` raise StanceAssessment 处，若来源步骤 `inferred_params_for_step` 非空，digest 追加一行：`⚠ 该结果依赖 LLM 推断的未确认参数：gene=THRSP（请人工确认参数正确再据此判定立场）`。不改 options/recommendation。
5. **判决引擎仍确定性**：封顶是机械规则；LLM 不参与判决。argument 判决核心逻辑不动（只在 capped_evidence_grade 加一路查询）。
6. **零回归**：无推断参数时行为完全不变（不 emit 事件、不封顶、digest 无告警）；现有全部测试不改且通过；无新依赖/新表。新增 event_type 向后兼容（旧事件无此类型，投影返回空）。

## 交付物
- `agent.rs`：infer_replace_params 返回 Vec<String>；apply 成功后 emit `agent.params_inferred`；StanceAssessment digest 告警；`inferred_params_for_step` 投影助手（或置于合适投影处）。
- `argument.rs`：capped_evidence_grade 增「来源步骤有推断参数 → 封顶」一路。
- 测试：溯源事件 emit + 投影读出；证据封顶（用 NON-exploratory 工具 + 推断参数，证明独立于 maturity 封顶生效）；无推断参数零封顶/零告警；digest 含告警；现有测试不改且通过。

## 验收标准（Claude 逐条复核）
- [ ] clippy -D warnings 干净；cargo test 全绿；acceptance 通过。
- [ ] **零回归铁证**：无推断参数路径行为不变；现有 core/cli 测试不改且通过；marker/tcga spec hash 不受影响（本步不碰 tool spec）。
- [ ] 溯源：循环推断参数 → emit `agent.params_inferred`，`inferred_params_for_step` 能读出。
- [ ] 封顶：NON-exploratory 工具 + 推断参数的 observation → 证据 Observed 被封顶 Inferred（行为测试）；无推断参数 → 不封顶。
- [ ] 可见：StanceAssessment digest 在来源步骤用推断参数时含 ⚠ 告警行。
- [ ] 仅 agentflow-core；无新依赖/新表；新增 event_type 向后兼容。

## 不在本里程碑
- 参数真实存在性在线核验（如基因符号查库）。
- 报告渲染推断参数溯源段（digest 已部分体现；报告专项后续）。
- 🟡 文献/数据孤岛（独立后续）。

## 验收记录（Claude 独立复验 2026-06-04）
- ✅ clippy -D warnings 干净；cargo test core 245(+5)/cli 57+2/schemas 3 全绿；acceptance 通过；fmt 通过。
- ✅ 溯源：const PARAMS_INFERRED_EVENT="agent.params_inferred"；emit 在 graph patch apply 成功后、auto-run 前；ProjectStore::inferred_params_for_step（兼容 local 与 step:<flow>/<step> id），测试断言 emit。
- ✅ 封顶：argument.rs source_inferred_params_for_observation(623)→capped_evidence_grade(609)，observation 来源步骤有推断参数则 Observed→Inferred，与 exploratory 封顶并存；测试用非-exploratory 工具证明独立生效。
- ✅ 可见：L4 StanceAssessment digest 在来源步骤有推断参数时追加 ⚠ 告警行（测试断言有/无两路）。
- ✅ 判决确定性守住：封顶机械规则，render_verdict 核心未动；仅 agent.rs+argument.rs；无新依赖/新表；不碰 tool spec（marker 2f8e22fc89c1caf9 定义性不变）。
结论：合并就绪。审计 🟠 参数信任缺口 B 面闭合——LLM 推断参数全程溯源、依赖未确认推断的证据机制封顶不冒充 Observed、交接 digest 可见告警。🟠 信任缺口（PV1 闸门 + PV2 溯源封顶）整体闭合。剩 🟡 文献/数据孤岛。
