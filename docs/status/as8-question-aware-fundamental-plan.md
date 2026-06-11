# AS8 实现简报：问题感知源筛选 + 诚实判研究空白(Fundamental)

Status: Assigned to Codex（feat/question-aware-fundamental,新 PR）
Owner: Claude(编排) · Codex(执行)
Spec source: 用户选定——真自主科研的诚实顶点:数据真不够时识别「研究空白」,不把代理当答案、不编造
Depends on: AS1-AS7(自治合成+源发现,AS7 在 feat/source-discovery 已提交但未合并→本分支从含 AS7 的提交起)

## 背景(live 实证)
AS7 源发现机制通,但 cBioPortal 有 MID1IP1 基因数据就被判 first-viable→落回它→生存代理。但 cBioPortal 无 ICB 免疫响应数据,**答不了免疫治疗问题**。诚实科学结论本就是「研究空白」。本里程碑让源筛选「问题感知」,数据真不够时诚实交接 Fundamental 研究空白。

## 编排者裁决（约束）
1. **问题数据需求抽取**:源发现前(或提议 prompt 内)让 LLM 明确「回答该假设需要什么数据」(如「ICB 治疗队列 + 响应标签 + 基因表达」),作为后续筛选基准。
2. **问题感知 viability**:候选源不仅要 allowlist 通过 + probe 返回非空,还要**该源含「问题所需数据」**。LLM 对每个候选标注「has_required_data: 是/否 + 理由」;probe 尽量验证问题特定数据存在。**只有「答得了该问题」的源才算 viable**。cBioPortal(有基因表达/生存,无 ICB 响应)→ 对免疫治疗问题判「related-but-insufficient」,不算 viable。
3. **诚实研究空白交接**:若无 viable 源(都 related-but-insufficient 或 probe 无问题特定数据),**不落回代理、不编造**;raise 一个研究空白决策点(新 DecisionKind 如 `FundamentalGap`,或复用 ToolGap 加类型),digest 含:问题数据需求、提议/探测了哪些源及各自为何不够、建议「这可能是 Fundamental 研究空白(需前瞻 ICB 队列等),请确认」。**判 Fundamental 是强声明→交人类确认**(A2),不自动独断;判决仍 inconclusive 直到人类确认。
4. **可见性(补 AS7 缺口)**:源发现全过程(问题需求、提议的源、probe 结果、viable/insufficient 判定)记入 cycle 输出 + 一个事件(如 `agent.source_discovery`),让用户能看到它试了什么。
5. **代理处理**:若存在「related-but-insufficient」源(如 cBioPortal 能做生存代理),digest 可注明「存在 X 代理分析但不直接回答本问题」,但**不把它当答案、不自动 run 当主证据**(避免代理冒充答案)。
6. **保留 AS1-AS7 全部**:no-fabrication、输入敏感性、运行时门、测试-修复、grounding、验证客户端复用、安全 allowlist、L4 ⚠ 未验证、0 判决权重、默认全开、可配置 LLM、dedup。判决确定性(Fundamental 经人类确认走既有 render_verdict+gate,不被 LLM 独断)。
7. core 零 LLM 依赖;无新 Rust 依赖。新 DecisionKind/event_type 向后兼容。

## 交付物
- synth_commands.rs / 发现模块:问题数据需求抽取 + 候选 has_required_data 标注 + 问题感知 viable 选择;无 viable→结构化「研究空白」信息(需求+源trace)。
- agent.rs:无 viable 源→raise FundamentalGap 决策点(digest 含需求+源trace+建议确认)；源发现 trace 入 cycle 输出 + emit agent.source_discovery 事件。
- handoff.rs:DecisionKind 增 FundamentalGap(as_str/parse)。
- 测试(离线 stub):问题感知筛选(有基因无问题数据的源→insufficient)；无 viable→raise FundamentalGap 含 trace;source_discovery 事件/输出可见;AS1-AS7 不回归。

## 验收标准（Claude 复核 + live）
- [ ] clippy/test/acceptance/fmt 全绿;AS1-AS7 加固与测试保持。
- [ ] 问题感知:有基因但无问题特定数据的源被判 insufficient(单测)；只有答得了的源 viable。
- [ ] 无 viable→raise FundamentalGap 决策点,digest 含 问题需求 + 探测的源 + 各自不够的理由 + 建议人类确认 Fundamental;不落回代理当答案、不编造。
- [ ] 可见性:cycle 输出/事件含源发现 trace。
- [ ] 判决确定性:系统不自动独断 Fundamental(交人类确认);render_verdict/gate 未被 LLM 触达。
- [ ] **live(编排者,真 DeepSeek+网络)**:纯 agent run MID1IP1 免疫治疗 → LLM 抽问题需求(ICB 响应)→ 提议/探测源 → cBioPortal 等被判 insufficient(无 ICB)→ 若全不够则 raise 研究空白决策点(含 trace)、诚实呈现「这是研究空白,需 ICB 队列」、不编造、不把生存代理当答案。
- [ ] core 无新依赖;新 DecisionKind/event 向后兼容。

## 不在本里程碑
- 自动 render Fundamental 落库(仍交人类确认);通用搜索引擎;任意源可复用客户端库治理。
