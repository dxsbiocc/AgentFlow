# AS3 实现简报：合成工具运行时契约打通(让自动合成的工具能真跑出真结果)

Status: Assigned to Codex（feat/auto-synth 上,与 AS1/AS2 同 PR）
Owner: Claude(编排) · Codex(执行)
Spec source: DeepSeek live 重测发现——合成工具过了验证却运行时失败(SYNTH_INPUT 未设),因合成只教了验证契约没教运行时契约
Depends on: AS1/AS2(同分支)

## 背景(live 实证的三处错位)
1. 合成工具 spec 模板(synthesized_tool_yaml)写死一个通用 `input` 参数,不是领域参数(gene 等)→ 运行时传 AGENTFLOW_PARAM_INPUT,工具不知分析哪个基因。
2. 合成 prompt 只教从 SYNTH_INPUT 读输入(那是验证 fixture 约定);运行时无 SYNTH_INPUT,工具 sys.exit。
3. 没教 LLM 运行时去真实数据源(cBioPortal REST)取数。
现有 tcga 工具的真实契约:读 AGENTFLOW_PARAM_<NAME>(如 AGENTFLOW_PARAM_GENE)→ 自己 fetch cBioPortal → 写 AGENTFLOW_OUTPUT_<OUTPUT>。

## 目标
让自动合成的工具按**双模契约**写:验证用 SYNTH_INPUT fixture 测计算逻辑(离线、确定、输入敏感);运行时读 AGENTFLOW_PARAM_* + 自己去真实数据源取数 + 写 AGENTFLOW_OUTPUT_RESULT。能取到真数据的缺口→真跑出真结果;取不到→诚实非零退出(不编造,AS2 保留)。

## 编排者裁决（约束）
1. **领域参数进 spec**:合成时让 LLM(或从假设/缺口)确定工具需要的领域参数(至少 `gene`),synthesized_tool_yaml 声明这些 param(而非通用 input);draft_synthesized_step 经 infer 填(gene=MID1IP1),运行时 runtime 传 AGENTFLOW_PARAM_GENE。可让 LLM 在候选里声明 params 列表,或固定从假设抽 gene 这类常见参数(择简单稳妥者,但运行时必须能把领域值传进工具)。
2. **合成 prompt 教运行时契约**(双模):
   - 运行时:从 AGENTFLOW_PARAM_<UPPER_NAME> 读参数(如 AGENTFLOW_PARAM_GENE);去真实公开数据源取数(给出 cBioPortal REST 基址 https://www.cbioportal.org/api 作参考,允许其它公开源);把结果写 AGENTFLOW_OUTPUT_RESULT 指向的文件路径。
   - 验证:若 SYNTH_INPUT 已设,则读该 fixture 文件、用同样计算逻辑产出(离线、确定);验证仍跑两份不同 fixture 做输入敏感性检测。
   - 二者都拿不到数据 → 非零退出,绝不编造(保留 AS2)。
   - 附 tcga_survival_assoc.py 的契约片段作 few-shot 参考(读 AGENTFLOW_PARAM_GENE / 写 AGENTFLOW_OUTPUT_*）。
3. **保留 AS2 全部加固**:输入敏感性检测、no-fabrication、L4 ⚠ 未验证 digest、合成证据 0 判决权重、默认全开、DeepSeek 默认 deepseek-v4-flash。
4. core 仍零 LLM 依赖;判决仍确定性;不新增 Rust 依赖。
5. 验证仍离线(SYNTH_INPUT fixture);**不要求验证阶段联网**(运行时才联网);因此一个工具可能验证过、运行时 fetch 失败 → 诚实失败(可接受)。

## 交付物
- synth_commands.rs：synthesized_tool_yaml 支持领域参数(gene 等);build_auto_synth_prompt 加双模运行时契约 + cBioPortal 参考 + tcga few-shot;候选若声明 params 则据此生成 spec。
- agent.rs：draft_synthesized_step 对领域参数 infer 填值(gene=假设里的基因);运行时透传。
- 测试：合成工具 spec 含领域 param(gene);stub 候选含读 AGENTFLOW_PARAM_GENE 的脚本→离线 fixture 验证过 + 运行时用 AGENTFLOW_PARAM_GENE 跑通(用本地可跑的 stub 工具模拟,不联网);输入敏感性、no-fabrication、0 权重保持。

## 验收标准（Claude 复核 + live）
- [ ] clippy/test/acceptance/fmt 全绿。
- [ ] 合成工具 spec 声明领域参数(gene),运行时收到 AGENTFLOW_PARAM_GENE(离线 stub 测)。
- [ ] 双模:SYNTH_INPUT 设→读 fixture(验证);未设→读 AGENTFLOW_PARAM_* 路径(运行时)。离线测覆盖两模。
- [ ] AS2 加固全部保留(输入敏感性/no-fabrication/⚠ digest/0 权重)。
- [ ] **live(编排者)**:DeepSeek 纯 agent run MID1IP1 → 合成工具读 AGENTFLOW_PARAM_GENE=MID1IP1、去 cBioPortal 取真数据;**真跑出真结果则 L4 含真发现+⚠ 未验证**;取不到真数据则诚实失败(不编造)。两种都算通过(关键是不编造 + 能用真数据时真跑通)。

## 不在本里程碑
- 强保证 LLM 一定写对某领域 API(本质难);硬安全沙箱;免疫治疗专用数据源对接(LLM 自行尝试公开源,失败则诚实退出)。
