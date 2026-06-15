# AS17 实现简报：泛化验证门（spec-级 cohort 推断 + 跨队列验证，CLI 侧、读为主、不改工具库）

Status: Assigned to Codex（新分支 feat/generalization-validation，从 main 起，main 已含 AS15+AS16+RFC）
Owner: Claude(编排) · Codex(执行)
Spec source: docs/design/tool-evolution-engine-design.md §4②③④ / §11.4-11.5 / §12（AS17，spec-级路径，编排者与维护者确认）
Depends on: AS1–AS16（含 AS16 的 `generalization_candidates`）

## 背景与范围裁决

AS16 已对 output-domain-mismatch surface「可泛化候选」(工具+I/O 指纹+peers)。AS17 做**质变前的验证门**：对候选,判断其"变异点"(领域绑定的运行时参数,如 cohort/study),从假设**推断正确 cohort**,用**真实 runtime gate 跨队列重跑验证**(原始默认 cohort + 推断 cohort),据 RFC §11.5 给出"可提升/拒绝"判定。

**关键裁决（已与维护者确认）**：
- **spec-级路径**：针对"运行时已支持参数(如 `AGENTFLOW_PARAM_STUDY`)但 AgentFlow 未暴露/未推断"的常见情形。**不改 examples/*.py 源、不做 LLM 源重构**。
- **CLI 侧、读为主**：在 `agent run` 跑完 cycle、拿到 `generalization_candidates` 后,做一个 **CLI 侧验证 pass**。**不改 core、不改工具库(不注册新版本/不晋升/不删除)、不新增 DecisionKind**。真正的"采纳/扬弃/谱系"留给 AS18(人在环治理门)。
- **验证门是安全网**：跨队列重跑任一失败 → 判"暂不可提升"并诚实报告原因(RFC §11.6 失败回退),绝不产出未验证的"已泛化"结论。

## 编排者裁决（约束）

### 1. 新 LLM seam（CLI 侧，复用已配置后端，Noop 默认）

- `CohortInferer`：`infer_cohort_study(hypothesis_statement) -> Option<String>` —— 从假设推断最匹配的 cBioPortal study id(如 "colorectal cancer" → 一个 coad/coadread study id);不确定返回 None。复用 RelevanceScorer 同一 LLM wiring。
- `VariationPointIdentifier`(或合进上面一个 LLM 调用)：判断候选工具的"领域变异参数名"。**首版可固化为 cohort/study 这一类**:即只处理"工具运行时读 `AGENTFLOW_PARAM_STUDY`(或声明了 study/cohort 参数)"的候选;识别方式 = 检查该工具 spec/runtime 是否涉及 study/cohort 参数(关键字)或运行时 env 约定。识别不出变异点的候选 → 跳过(不验证、如实标注"变异点未识别")。

### 2. 验证 pass（CLI 侧，runtime gate 跨队列重跑）

对每个 `generalization_candidate`(来自 cycle report):
1. 取候选 `hypothesis_id` → 该假设 statement(`inspect_hypothesis`)。
2. `CohortInferer.infer_cohort_study(statement)` → 推断 cohort study id;None → 标"cohort 未推断,暂不可提升",跳过。
3. **跨队列验证(RFC §11.5:原始+新, 首版 K=0 即只这两个)**：用候选工具的 runtime(`executable_tool`/已有 `run_python_script` 机制)各跑一次:
   - **原始 cohort**(工具当前默认,如 lihc)——回归,证无损;
   - **推断 cohort**(新,如 coad)——证新能力;
   - 两次都设 `AGENTFLOW_PARAM_GENE`(从假设推断的 gene,复用现有 infer)与 `AGENTFLOW_PARAM_STUDY`=对应 cohort;经既有 runtime gate(非空、字段合理、no-fabrication 不回归)。
4. 判定:两次都过 → `verdict=promotable`(诚实附带两次的简短证据);任一失败 → `verdict=rejected` + 失败 cohort + 原因(如"coad study 不可达/字段缺失")。

### 3. 输出（读为主，非阻塞）

- 在 `agent run` 文本输出新增一段:`🧪 泛化验证: <tool_ref> — cohort 参数化 [promotable: lihc✓ + coad✓ | rejected: coad ✗ <reason> | skipped: <原因>]`。
- JSON：可在 CLI 层组装一个 `generalization_validations` 数组随 agent run JSON 输出(CLI 侧结构,不动 core 的 CycleReport)。
- **不持久化、不注册、不晋升**——纯报告。AS18 据此(或重算)做人在环采纳。

### 4. 不变量与约束

- **不改 core**(agent.rs/CycleReport 不动);不碰 `argument.rs`。
- **不改工具库**:不 register/不晋升/不删除/不写 spec;只做**临时验证运行**(等同既有 runtime gate 的临时执行,不落库)。
- 不新增 DecisionKind/持久事件。
- CohortInferer Noop 默认 → 未配置 LLM 时本 pass 不产判定(skipped),零回归。
- 验证运行复用 AS10 的 seatbelt/no_proxy 与 AS11 的代理感知(走既有 run_python_script 路径)。
- 核心/CLI 无任何具体基因/疾病/study 常量写死(cohort 由 LLM 从假设推断;original cohort 从工具自身默认读取,不在 AgentFlow 写死)。
- 无新依赖。

## 测试（离线 stub）

`agentflow-cli`:
- stub `CohortInferer` 返回一个固定 study + stub runtime(两次都"成功")→ `verdict=promotable`,输出含两 cohort ✓。
- stub runtime 让"新 cohort"失败 → `verdict=rejected` + 原因 + 失败 cohort。
- `CohortInferer` 返回 None → `skipped: cohort 未推断`。
- 变异点识别不出(候选工具与 study/cohort 无关)→ `skipped: 变异点未识别`。
- Noop CohortInferer → 无判定(skipped),既有 agent run 行为零回归。
- 既有 cli 测试保持绿。

## 验收标准（Claude 复核 + live）

- [ ] fmt / clippy / core / cli / `scripts/acceptance-v1.sh` 全绿；`argument.rs` 与 core CycleReport **未改动**。
- [ ] 单测覆盖 promotable / rejected / skipped(cohort 未推断 / 变异点未识别)/ Noop 零回归。
- [ ] **live(编排者,纯 `agent run`,不干预)**：对结直肠/肺等假设产生的 survival_assoc 泛化候选,系统推断 cohort、**跨 lihc+新队列真实重跑验证**,给出 promotable/rejected 判定并诚实呈现;**不改任何工具**。
- [ ] 不改工具库/不注册/不晋升;不新增 DecisionKind;无新依赖;无写死领域常量。

## 不在本里程碑

- 不采纳/不注册泛化工具版本、不晋升 maturity、不扬弃旧特化、不写谱系 —— AS18(人在环治理门)。
- 不做 LLM 源级重构(只 spec-级 cohort 参数);不做 K>0 抽样(首版只验原始+新)。
- 不自动用正确 cohort 重跑产证据(那是采纳后的事,AS18+)。
- 不改 core / argument.rs / examples 工件。
