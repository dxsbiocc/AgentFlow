# AS6 实现简报：验证过的 cBioPortal 客户端供合成工具复用（用库不造轮子）

Status: Assigned to Codex（feat/synth-test-fix 上,与 AS4/AS5 同 PR）
Owner: Claude(编排) · Codex(执行)
Spec source: 用户选定「提供验证过的 cBioPortal 客户端供复用」——深 API 长尾的正解
Depends on: AS1-AS5(自治合成 + 测试-修复 + grounding,本分支)；tcga_survival_assoc.py(已验证能正确访问 cBioPortal)

## 背景
AS1-AS5 证明:从零自动写深 API(cBioPortal)客户端是细节长尾(ID→join→HTTP方法→…),每轮修一层。正解=别让 LLM 重复造轮子:给它一个验证过的客户端,只写分析逻辑。tcga_survival_assoc.py 已含正确的 cBioPortal 访问(study/profile/sample 解析、molecular-data 与 clinical-data 的正确 POST 取数)。

## 目标
抽出验证过的 cBioPortal 客户端 Python 模块,让合成工具 import 它取数(不再自写 HTTP/API),只写分析逻辑。客户端在验证(运行时门)与真实运行时都可被合成工具 import。

## 编排者裁决（约束）
1. **客户端模块**(从 tcga_survival_assoc.py 忠实抽取,保持其已验证的正确访问):放一个稳定模块,如 `examples/tools/agentflow_cbioportal.py`(或合适的随项目分发位置),暴露干净函数,至少:
   - `resolve_study(cancer_keyword) -> study_id`(偏好 pan_can_atlas)
   - `fetch_expression(study_id, gene) -> {sample_id: value}`(mRNA 表达,正确 molecular profile + POST molecular-data)
   - `fetch_overall_survival(study_id) -> {patient_id: (os_months, os_event)}`(临床数据正确 POST)
   - 可选 `fetch_clinical_attribute(study_id, attr)`。
   - 只用 Python 标准库(urllib),与现有工具运行环境一致。
2. **客户端对合成工具可见**:合成工具在 validation 运行时门 与 真实运行时 都能 `import agentflow_cbioportal`。机制:把该模块放到合成脚本运行的 workdir / 或加其目录到 PYTHONPATH(run_python_script 运行候选时设 PYTHONPATH 含客户端目录;真实 runtime 同样)。确保两处一致。
3. **合成 prompt 改为"用客户端"**:build_auto_synth_prompt 告诉 LLM「有验证过的客户端可用:`import agentflow_cbioportal`,用 resolve_study/fetch_expression/fetch_overall_survival 取数,**不要自己写 HTTP/API 调用**;你只写分析逻辑(关联/分组/统计),从 AGENTFLOW_PARAM_GENE 读基因,写 AGENTFLOW_OUTPUT_RESULT」。附客户端函数签名+docstring。仍保留双模(SYNTH_INPUT fixture 离线验证)与 no-fabrication。
4. **保留 AS1-AS5 全部**:no-fabrication、输入敏感性、运行时门、测试-修复循环、grounding(可与客户端并存:grounding 仍提供 study 等真实 ID 作参考)、L4 ⚠ 未验证、0 判决权重、默认全开、可配置 LLM、dedup。
5. **诚实范围**:客户端只提供 cBioPortal 真有的(表达/生存/临床属性)。免疫治疗 ICB 响应数据 TCGA 没有→合成工具能做表达-生存关联(真结果),免疫解读由人在 L4 做。客户端不编造任何数据;取不到→抛错(工具据此诚实失败)。
6. core 零 LLM 依赖;判决确定性;无新 Rust 依赖。客户端是 Python 模块(随工具运行,不进 Rust)。

## 交付物
- examples/tools/agentflow_cbioportal.py：验证过的客户端(从 tcga 工具抽取,含自测 main 或 docstring 示例)。
- synth_commands.rs：run_python_script 给候选设 PYTHONPATH 含客户端目录(validation 与 runtime 一致);build_auto_synth_prompt 改为"用客户端"+附签名;grounding 仍注入。
- 测试(离线 stub):合成工具能 import 客户端(stub 客户端模拟 fetch,不真联网)→ 验证过;prompt 含客户端 import 指引;AS4/AS5 不回归。

## 验收标准（Claude 复核 + live）
- [ ] clippy/test/acceptance/fmt 全绿;AS1-AS5 加固与测试保持。
- [ ] 客户端模块独立可用(Claude 会直接 python 调它取 MID1IP1 真数据验证其正确)。
- [ ] 合成工具能 import 客户端(离线 stub 测);prompt 指引"用客户端不自写 API"。
- [ ] **live(编排者,真 DeepSeek+cBioPortal)**:纯 agent run MID1IP1 → 合成工具 import 客户端取真数据 → 运行时门过 → **真跑出 MID1IP1 在 LIHC 的表达-生存关联真实结果** → L4 含真发现 + ⚠ 未验证(合成工具)。这是首次真跑出真科学结果。
- [ ] core 无新依赖;判决确定性;客户端不编造。

## 不在本里程碑
- 非 cBioPortal 源的客户端;ICB 专用数据源对接;硬安全沙箱;客户端成为正式可复用工具库的完整治理(先一个模块)。
