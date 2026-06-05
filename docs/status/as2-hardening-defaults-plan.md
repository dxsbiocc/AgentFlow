# AS2 实现简报：加固自治合成(堵编造洞) + 默认全开流程 + 修 LLM 配置

Status: Assigned to Codex（在 feat/auto-synth 上,与 AS1 同 PR）
Owner: Claude(编排) · Codex(执行)
Spec source: live MID1IP1 测试暴露「合成工具会编造数据」严重自欺洞 + 用户三点(加固/默认全开/用配置LLM不锁claude)
Depends on: AS1(79bea50, 同分支)、S2 封顶、L4、llm_commands.rs(已有 provider 配置)

## 背景(live 实证的严重问题)
MID1IP1 免疫治疗场景 `--auto-synth` live 跑:AgentFlow 自治合成了工具,但该工具**取不到真数据就回退硬编码 DEFAULT_PANEL**,产出整份**编造**的报告(HR=1.42/p=0.018/cd8_corr=-0.21/"MODERATE biomarker 证据"),冒烟 fixture 照过,L4 还称其"产出真实发现"。grade 封顶挡住了"假 affirmed",但编造报告+误导措辞仍能骗人。这是项目最该防的自欺,被自治合成开了后门。

## 目标:三件事一起做
① 加固合成安全(堵编造洞) ② agent run 默认跑整个智能流程(不靠逐个 flag) ③ 修 DeepSeek 配置 + 默认走配置的 LLM。

## ① 加固合成（堵编造，安全敏感，重点）
1. **合成 prompt 强约束**:明确要求生成的工具"必须从真实数据源获取/计算;**禁止硬编码或编造数值;真实数据不可得时必须 loudly 失败(非零退出),不许 'default'/'illustrative' 回退**"。
2. **硬编码/输入不变性检测(验证闸,核心)**:验证阶段用**≥2 份有意义不同的 fixture 输入**跑候选工具;若输出**不随输入变化**(归一化掉时间戳等易变位后相同)→ 判定"疑似忽略输入/编造" → **验证不通过,不注册不跑**。诚实标注:此检测抓的是「输出与输入无关」这种 blatant 编造(如 MID1IP1 的 DEFAULT_PANEL),**不能保证消除所有编造**,只是减少;真兜底仍是封顶+溯源+人类核验。
3. **溯源诚实(L4 digest)**:步骤用了自动合成工具时,digest **绝不说"产出真实发现"**;改为 ⚠「步骤 <id> 使用【自动合成的未验证工具 <ref>】产出结果。该工具由 LLM 生成、仅过冒烟+输入敏感性检测,可能仍含编造/硬编码。请先核验工具逻辑与数据来源,再判定立场。」
4. **合成证据更低权重**:合成工具证据在判决中权重压到最低(维持 S2 不能 affirm 的同时,确保未经人类核验前对判决分数贡献≈0)。实现可复用现有 grade/cap,或加"unverified_synthesized"标记使 score_for 近零计;**不引入会破坏现有判决测试的改动,优先用标记+封顶**。

## ② 默认跑整个流程（usability，翻转 opt-in→opt-out）
- `agent run` **默认开启**:apply + auto_run + auto_flow + infer_params + semantic_match + auto_forage + auto_synth(全部智能)。
- 提供**关闭开关**:`--no-apply`/`--no-auto-run`/`--no-infer-params`/`--no-semantic-match`/`--no-auto-forage`/`--no-auto-synth`;并加 `--dry-run`(= 全关,仅提议)。
- 现有依赖"默认关"的测试相应更新为新默认语义(语义演进,非回归 bug);保留对"关闭开关→旧行为"的覆盖。

## ③ LLM 配置修复 + 默认走配置
- 修 DeepSeek 默认模型:`deepseek-v4-flash`(不存在)→ `deepseek-chat`。
- 确认 `agent run` 无 `--synthesizer` 时走 `configured_or_default_synthesizer`(已是,验证保持):项目配了 LLM(如 deepseek)就默认用它,没配才退 `claude -p`。
- llm config 写入 `.agentflow/`(已 gitignore),key 不上库;校验 provider/model/key 存在。

## 编排者裁决（约束）
- core 仍零 LLM 依赖(LLM 全走 CLI 子进程/配置脚本)。判决仍确定性。
- 加固不得削弱现有 PV/AF1/L4/封顶;默认翻转后,关闭开关能完全复现旧行为(回归可控)。
- 不新增 Rust 依赖(API 调用走已有的 llm-synth.py urllib 方案)。

## 验收标准（Claude 复核 + live）
- [ ] clippy/test/acceptance/fmt 全绿。
- [ ] **硬编码检测**:stub/离线测——输出不随输入变的候选被拒、随输入变的通过。
- [ ] **digest 诚实**:合成工具步骤的 stance digest 含 ⚠ 未验证措辞,无"真实发现"。
- [ ] **默认全开**:`agent run`(无任何功能 flag)端到端跑 infer+match+synth+run+forage;`--dry-run` 仅提议;`--no-auto-synth` 等关闭生效。
- [ ] DeepSeek 默认模型 = deepseek-chat;agent run 默认走配置 LLM(无 --synthesizer 时)。
- [ ] **live(编排者做)**:配 DeepSeek 后,纯 `agent run` MID1IP1 → 若合成工具编造(输出不随输入变)被拒;真合成则 digest ⚠ 未验证、证据近零权重、判决不被编造数据带跑。

## 不在本里程碑
- 强正确性验证(本质难);硬安全沙箱;非 input-invariance 的更复杂编造检测(后续)。
