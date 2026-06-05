# AS5 实现简报：合成前活体 API 发现 grounding（学 API，提升合成准确性 角度②b）

Status: Assigned to Codex（feat/synth-test-fix 上,与 AS4 同 PR）
Owner: Claude(编排) · Codex(执行)
Spec source: 用户研究模型「检索→学 API→测试」+ 选定 ②b 活体 API 发现
Depends on: AS1-AS4(自治合成 + 测试-修复循环,AS4 在本分支)

## 背景
AS4 测试-修复循环能逐轮修,但 3 轮没把 cBioPortal 的 study/profile/sample-list ID 写对(LLM 凭记忆猜 ID)。人类会先查 API 的真实可用 ID 再写。本里程碑在合成前**活体查询数据源发现端点**,把真实 ID 注入 prompt,让 LLM 据真实事实写代码,第一次就写对 → AS4 运行时门一次过。

## 目标
合成前做一次「API 发现」:对癌症基因组类缺口,查 cBioPortal 发现端点拿真实 study/molecular-profile/sample-list ID,注入合成 prompt。发现失败则跳过(test-fix 循环兜底)。结构可扩展(先 cBioPortal,后续可加别的源)。

## 编排者裁决（约束）
1. **发现函数(CLI,合成前,联网)**:`discover_cbioportal_grounding(hypothesis_statement) -> Option<String>`:
   - 从假设抽癌种词(liver/hepatocellular/LIHC 等),查 `https://www.cbioportal.org/api/studies`,按癌种名/cancerType 匹配最合适 study(偏好 pan_can_atlas 类),拿 studyId。
   - 查该 study 的 `/molecular-profiles` 找 mRNA 表达谱 id;`/sample-lists` 找合适样本列表 id(如 *_all)。
   - 返回格式化事实块:「Discovered real cBioPortal identifiers for this hypothesis: studyId=<>, mrnaMolecularProfileId=<>, sampleListId=<>, api_base=https://www.cbioportal.org/api。Use these EXACT identifiers; do not guess.」拿不到 → None。
   - 带超时;任何失败/无匹配 → None(不报错,不阻断)。
2. **注入合成 prompt**:build_auto_synth_prompt 在有 grounding 时把事实块放在显著位置,要求 LLM 用这些确切 ID。无 grounding 时维持原 prompt(few-shot)。
3. **保留 AS1-AS4 全部**:no-fabrication、输入敏感性、运行时门、测试-修复循环(N=3)、L4 ⚠ 未验证、0 判决权重、双模契约、默认全开、可配置 LLM、dedup。grounding 是“先给真实 ID 让一次写对”,test-fix 仍兜底。
4. core 零 LLM 依赖;判决确定性;无新 Rust 依赖(发现用已有 python/urllib 子进程或 CLI 内 std)。发现是 CLI 侧网络调用。
5. 范围诚实:本步只做“查已知源(cBioPortal)的发现端点”;“网络检索找到该用哪个 API”是更上游一步,不在本里程碑。

## 交付物
- synth_commands.rs / 新 discovery 模块：discover_cbioportal_grounding(联网查发现端点) + 注入 build_auto_synth_prompt。
- 测试(离线 stub,不真联网):mock 发现端点响应 → grounding 事实块正确生成并注入 prompt;发现失败/无匹配 → None 且合成照常(few-shot);AS4 测试-修复/运行时门/dedup 不回归。

## 验收标准（Claude 复核 + live）
- [ ] clippy/test/acceptance/fmt 全绿;AS1-AS4 加固与测试保持。
- [ ] 发现函数单测(mock):正确抽癌种、选 study、拿 profile/sample-list、生成事实块;失败→None 不阻断。
- [ ] **live(编排者,真 DeepSeek+cBioPortal)**:纯 agent run MID1IP1 → 合成前查到真实 cBioPortal ID 注入 → DeepSeek 据此写对数据获取 → 运行时门(可能一次)过 → **真跑出 MID1IP1 的 cBioPortal 真实结果**(expression-survival 关联)→ L4 含真发现 + ⚠ 未验证;若仍不过则 test-fix 兜底,最终诚实(不编造)。
- [ ] core 无新依赖;判决确定性。

## 不在本里程碑
- 网络检索“找到该用哪个 API”(更上游,后续)；非 cBioPortal 源的发现 recipe(可扩展结构留好,实现先 cBioPortal)；硬安全沙箱。
