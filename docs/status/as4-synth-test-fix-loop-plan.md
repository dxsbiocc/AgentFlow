# AS4 实现简报：合成测试-修复循环 + 运行时门 + 成功即留(准确性提升 ①+③)

Status: Assigned to Codex
Owner: Claude(编排) · Codex(执行)
Spec source: 用户「从不同方面提升 LLM 撰写工具准确性」+ 选定混合封装「成功即提升为复用库」。模型=人类:检索→学API→不断测试改对。
Depends on: AS1-AS3.1(自治合成,已合并 main)

## 背景
AS3.1 后合成工具能读真参数、打真 cBioPortal API,但**一次性合成**:LLM 把 study-id 细节(lihc_tcga vs lihc_tcga_pan_can_atlas_2018)写错→运行时诚实失败,但不会自我修正。人类会「不断测试改对」。本里程碑给合成加测试-修复循环 + 运行时门;并实现「成功即留」(只有真跑通的工具才注册保留→未来匹配自然复用=混合封装)。

## 目标
合成器内部:离线门(防编造/输入敏感,已有)→ **运行时门**(用真参数+真取数跑一次)→ 失败则把错误喂回 LLM 重写,最多 N 次 → 过了运行时门才注册。只有真能取到数据的工具被保留,坏的不注册。

## 编排者裁决（约束）
1. **测试-修复循环**(在 LlmToolSynthesizer.synthesize 内,CLI):
   - 迭代(最多 N=3):a) LLM 产出候选 → b) 离线门(SYNTH_INPUT 双 fixture 输入敏感性 + no-fabrication,已有) → c) **运行时门**:用真实代表参数(传入的推断 gene,如 MID1IP1)+ 真取数(AGENTFLOW_PARAM_GENE + 网络)跑候选,带超时;捕获 stdout/stderr/exit。
   - 运行时门**失败** → 把 {错误信息, 上一版候选代码} 拼进下一轮 prompt:「你写的工具运行失败,错误如下:<stderr>。这是代码:<code>。修正它,仍遵守[no-fabrication/双模契约]」→ 回 a)。
   - 运行时门**过**(exit 0 且输出非空且非编造) → register(exploratory),返回 tool_ref。
   - N 次仍不过 → Rejected(最后错误),不注册(诚实,无坏工具留存)。
2. **运行时门联网**:这是「不断测试」的必然。带超时(如 60s/轮);网络不通→该轮失败→最终 Rejected(可接受)。离线门仍先跑(防编造在联网前就拦)。
3. **成功即留 = 混合封装**:只有过运行时门的工具被注册;注册后未来缺口经现有 match_tools 自然复用(同假设/同领域再来时命中已注册的合成工具,不重复合成)。**确认 dedup**:同一假设已存在可用的合成工具时,优先复用而非再合成。
4. **保留 AS1-AS3.1 全部加固**:no-fabrication、输入敏感性、L4 ⚠ 未验证 digest、合成证据 0 判决权重、双模契约、默认全开、可配置 LLM。
5. synthesize 签名按需加「推断参数(gene 值)」入参,供运行时门用真 gene 测试。core 仍零 LLM 依赖;判决确定性;不新增 Rust 依赖(网络走已有 urllib 子进程/python)。
6. 每轮 LLM 调用 + 运行有耗时,N 小(3),带超时,避免无界。

## 交付物
- agent_ops_commands.rs / synth_commands.rs：synthesize 内的测试-修复循环 + 运行时门(真参数真取数) + 错误喂回 prompt;成功即注册,失败 N 次拒绝。
- agent.rs：把推断的 gene 值传给 synthesize(供运行时门);同假设已有可用合成工具则复用不重合成。
- 测试(离线 stub,不真联网):stub 合成器/候选——第一版「运行时门」失败(模拟错误)、把错误喂回后第二版通过→注册;N 次全失败→Rejected 不注册;成功的合成工具被后续匹配复用(dedup)。运行时门用本地可跑 stub 模拟「真取数」,不实际联网。

## 验收标准（Claude 复核 + live）
- [ ] clippy/test/acceptance/fmt 全绿;AS1-AS3.1 加固与测试保持。
- [ ] 测试-修复循环单测:失败→喂回→修正→过;N 次失败→拒绝不注册。
- [ ] 成功即留 + dedup 单测:过运行时门的工具被注册并被后续匹配复用,不重复合成。
- [ ] **live(编排者,真 DeepSeek+cBioPortal)**:纯 agent run MID1IP1 → 合成工具首版 study-id 错→循环喂回错误→DeepSeek 改对→运行时门过→真跑出 MID1IP1 的真实 cBioPortal 结果 → L4 含真发现 + ⚠ 未验证;或 N 次仍不过则诚实拒绝(不编造)。
- [ ] core 无新依赖;判决确定性。

## 不在本里程碑
- API 文档/发现端点检索注入 prompt(grounding 角度 ②,作为 AS5 后续)。
- 跨项目/全局可复用工具库(本步复用限本项目注册表)。
- 硬安全沙箱。
