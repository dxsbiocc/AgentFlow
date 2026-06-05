# AS1 实现简报：工具合成自治化（缺口→自动造工具→fixture冒烟→跑→封顶→人类解读）

Status: Implemented locally · PM verified 2026-06-05
Owner: Claude(编排) · Codex(执行)
Spec source: 用户批评「遇缺口就 punt 给人,说好的自动创建工具呢」+ 选定「自动造+fixture把关+跑+封顶」
Depends on: S1(synth: LLM写码+fixture验证+注册exploratory)、S2(exploratory grade 封顶)、S3(tool_gap)、AF1(auto-flow)、L4(stance)、PV2(溯源样板) —— 均在 main

## 背景（三个真缺口，已用 MID1IP1 场景证据坐实）
1. 缺口检测只认「零工具匹配」(agent.rs:340 matched_tool.is_none())。低 fit 工具会被硬跑——survival 工具拿去答免疫治疗问题。
2. 合成只在零匹配时 propose；低 fit「匹配」时跳过。
3. 即便 propose，也只「提议→等人批准」(S3)，非自动创建。

## 目标
让循环遇能力缺口时**自主造工具补上**:gap→LLM 写工具代码→fixture 冒烟验证→注册 exploratory→自动跑→证据封顶(S2)→**只把结果解读交人类**(L4)。人类决策从「批准造工具」挪到「解读结果」(同 AF1 把决策挪到 L4)。opt-in，默认关零回归。

## 安全模型（诚实，不可吹成正确性保证）
- **fixture 冒烟**:LLM 自生成 fixture+expect，只验「能跑、输出非空、含预期形状」——抓崩溃/空/畸形，**不保证科学正确**。
- **真安全网**:① exploratory grade 封顶(S2 已有)→合成证据永不能独立 affirm ② 人类在 L4 解读 ③ 溯源标记「自动合成」让人看到 caveat ④ 有限隔离:env_clear + 最小 PATH + workdir + 60s 超时 + Unix 进程组 best-effort 清理（不是硬沙箱）。
- 执行 LLM 写的代码有固有风险，与手动 synth 同档，仅少逐次批准——靠封顶+溯源+人类把关，不靠假验证。

## 编排者裁决（约束）
1. **缺口检测扩展**(agent.rs)：gap = `matched_tool.is_none()` **或 best 候选 fit==Low`。低 fit 不再硬跑，视为缺口(可配阈值；默认 Low 触发)。
2. **core trait 缝**(镜像 ParamInferer/RelevanceScorer，dep-free)：
   `ToolSynthesizer::synthesize(...) -> ToolSynthesisOutcome`，区分 `Registered { tool_ref }` 与 `Rejected { reason }`，让失败原因进入循环报告；`NoopToolSynthesizer` 返回 rejected。
3. **CLI 实现 LlmToolSynthesizer**(agent_ops_commands.rs，复用 synth_commands 的 run_synthesizer/validate_candidate_script/register)：从假设+缺口构 prompt → LLM 产出「工具脚本 + fixture 样例 + expect」(结构化三段) → 写脚本 → 冒烟 validate(先 fixture，再 runtime-like 无 SYNTH_INPUT；stdout/result 含 expect、非空；env_clear) → 过则 register_tool(maturity=exploratory) 返回 tool_ref；不过则 Rejected(不注册、清理候选、记 apply_failures)。
4. **循环集成**(run_cycle，新 opt-in `--auto-synth`)：缺口且 config.auto_synth → 先做预算/策略 preflight，允许落地才调用 synthesizer.synthesize → Registered(tool_ref) 则用它 draft_step→(复用 AF1)auto-flow→auto-run→证据封顶(S2 自动)→**溯源**(标记该步用了自动合成工具)→L4 raise stance_assessment，digest 注明「结果来自自动合成工具(冒烟验证、exploratory 封顶)，请人工确认工具是否真答了问题」。Rejected(reason) 则保留 proposal 并写 apply_failures，便于诊断。
5. **溯源**：新增事件或复用 PV2 模式标记「auto_synthesized tool=<ref> for step=<id>」；L4 digest 体现。
6. **零回归**：不传 --auto-synth → 行为完全不变(缺口检测扩展也仅在 auto_synth 下改触发，或对 propose-synth 维持原零匹配语义——确保现有测试不改且通过)。判决/PV/AF1/L4 逻辑不变。core 无新依赖(LLM 在 CLI 子进程)。

## 交付物
- agent.rs：缺口检测扩展(low-fit)；ToolSynthesizer trait+Noop；run_cycle_with 增 synthesizer 形参链(或 run_cycle_with_synth)；缺口→synthesize→draft→auto-flow→run→溯源→L4。
- agent_ops_commands.rs：LlmToolSynthesizer；--auto-synth flag 接线。
- synth_commands.rs：把 register/validate 逻辑暴露为可复用(pub(crate))，供 LlmToolSynthesizer 调。
- 测试：stub ToolSynthesizer 返回一个本地可跑的小工具 → 缺口→合成→注册→跑→证据封顶 Inferred→L4 含「自动合成」caveat；Rejected→不注册不跑且报告 apply_failures；--auto-synth 关→零回归；低-fit-当缺口；预算 preflight；validation env/runtime parity。

## 验收标准（Claude 复核 + live）
- [x] clippy/test/acceptance/fmt 全绿；默认关零回归(现有测试不改)。
- [x] stub 合成器行为测试(成功路径封顶+溯源+L4 caveat / Rejected 路径 / 低fit当缺口 / --auto-synth 关零回归 / budget preflight / validation env+runtime parity)。
- [ ] **live(真 claude)**:MID1IP1 免疫治疗假设 + --auto-synth → 识别 survival 工具不对题(缺口) → 自动合成一个针对免疫治疗的工具(如关联免疫 checkpoint/浸润) → fixture 冒烟过 → 注册 exploratory → 自动跑 → 证据 Inferred 封顶 → L4 交人类解读且 digest 注明自动合成。
- [x] 判决仍确定性；合成证据不能独立 affirm(S2 封顶生效)。
- [x] core 无新依赖。

## 不在本里程碑
- 硬安全沙箱(网络隔离/容器)——合成工具常需联网取数据，本步用 env_clear+最小 PATH+超时+workdir+best-effort 进程组清理+封顶+人类把关，硬沙箱另议。
- fixture 的强正确性验证(本质难，靠封顶+人类兜底)。
