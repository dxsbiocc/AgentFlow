# 设计文档：工具进化引擎（Tool Evolution Engine）

Status: DRAFT / RFC（供逐条定护栏后再分阶段实现，不是实现计划）
Author: Claude(编排) · 与维护者讨论稿
Scope: agentflow-core 的工具/合成/控制循环层
关联：触发器来自"输出领域校验"（B）；与 AS1–AS8 自主合成、AS13 复现谱系、tool maturity 本体咬合

---

## 0. 一句话

让工具**生而特化、用而进化**：一个为单次任务合成的特化工具，在被多次证明是同一能力的不同参数后，**经验证后提升为参数化的通用工具**（specialized → concrete-universal），使工具库随使用**收敛变强**，而不是随任务**累加变噪**。

## 1. 动机：从一个真实事件说起

闭环验证（SPP1/LUAD）暴露的事实：
- 引擎正确匹配 `tcga/survival_assoc`、真查 cBioPortal、得到**真实显著结果**（logrank p=0.000049）。
- 但工具把队列 **硬编码成 `lihc_tcga_pan_can_atlas_2018`（肝）**（`examples/tools/tcga_survival_assoc.py` 第134行 `--study` 默认值），而假设问的是 **LUAD（肺）**。右基因、错癌种。
- 该工具是为最初 THRSP-HCC（肝）任务而生的**单次任务工件**，只参数化了 `gene`、没参数化 `cohort`。

**两条错误的"修法"对照**：
- ❌ **累加式特化（曾提议的 C1）**：拉黑旧工具 → 合成一个 `survival_assoc_luad`。结果：工具库膨胀成 N 个一次性脚本，每个都可能藏同类 bug——把"单次任务代码当核心"的反模式乘以 N。
- ✅ **进化式收敛（本设计）**：识别"survival(lihc) 与 LUAD 需求是同一能力的不同 cohort 参数" → 把硬编码 cohort **提升为参数** → 跨队列重验 → 得到一个 `survival_assoc(gene, cohort)` 通用工具，旧特化被**扬弃**（保留+否定+提升）。

## 2. 辩证基础（决定机制形态，非修辞）

| 原理 | 在本引擎里的落点 |
|---|---|
| **矛盾为动力** | 工具的"特殊出身"(为肝而生) vs 研究的"普遍要求"(下一题是肺)——这个内在矛盾驱动进化。回避矛盾=再造特化；解决矛盾=提升为普遍。 |
| **否定之否定** | 抽象知识(生存关联,不可执行) → 否定为具体特化(LIHC,可执行但狭隘) → 否定之否定为**具体的普遍**(`survival(gene,cohort)`,可执行又普遍)。螺旋上升,非回到抽象。 |
| **量变到质变** | 同能力被特化使用 N 次(肝、肺…)的量积累,越过阈值触发**种类的跃迁**(脚本→通用能力)。引擎=检测"量是否够质"的扳机。 |
| **共性寓于个性** | 通用工具**从真实个别中萃取**(真跑过的工具+真出现的需求),非预先设计。实践先于抽象——这也说明 LIHC 硬编码不是错,**没进化它**才是。 |
| **扬弃(Aufhebung)** | 泛化=保留(验证过的分析逻辑+runtime-gate 血统)+否定(LIHC 限制)+提升(参数化)。旧工具不删除而被统摄,需**版本谱系**。 |

## 3. 架构总览：两个咬合的循环

```
                      ┌─────────────────────────────────────────┐
   假设 → 匹配工具 → 跑 →│  B. 输出领域校验 (矛盾的暴露)            │
                      │  读真实输出 scope vs 假设领域            │
                      └───────────────┬─────────────────────────┘
                          match │ mismatch
                          ↓     │
                    正常 stance  │  ┌──────────────────────────────┐
                    /证据/判决   └─→│  自纠正分叉                    │
                                   │  近邻工具? ──yes──> 进化(泛化)  │
                                   │            └─no───> 合成全新    │
                                   └──────────────┬───────────────┘
                                                  ↓
   ┌───────────────────────────────────────────────────────────────┐
   │  进化引擎 (矛盾的高阶解决, 也可由跨任务复发独立触发)             │
   │  ① 复发/相似检测 → ② 变异点识别 → ③ 泛化合成 →                  │
   │  ④ 验证门(跨案例重跑 runtime-gate) → ⑤ 扬弃+谱系 → ⑥ 人在环治理 │
   └───────────────────────────────────────────────────────────────┘
```

- **B = 矛盾暴露**：让 agent 读自己的输出，发现 lihc≠LUAD（详见 §5）。
- **自纠正分叉**：不再二元"拉黑/重造"，而是 **优先进化近邻工具**，无近邻才合成全新。
- **进化引擎**：既被 B 触发，也可被**跨任务复发**独立触发（即使没有 mismatch，发现一个能力被特化用了 3 次，也主动提议泛化）。

## 4. 进化引擎子机制

### ① 复发 / 相似检测（量变累积）
- **能力指纹（capability fingerprint）**：对每个工具/合成需求构造一个结构化指纹 = `{desired_output_type, required_input_types, 领域动作(survival/correlation/...), 变异槽(gene, cohort,...)}`。来源：工具 spec(I/O 签名) + 合成时的 capability_need 文本(语义)。
- **相似判定 = 多重 grounding，不单凭 LLM**：① I/O 签名结构等价(强信号) + ② 语义同能力判断(LLM seam，类比 `RelevanceScorer`) + ③ 实际使用证据(同一工具被不同假设以不同参数调用 ≥ 阈值)。三者**合取**才算"同能力候选"。
- **阈值（三次法则）**：不在第一次复发就泛化；累计 ≥ N(默认3) 个**已验证**的特化使用、或 B 检出的明确 mismatch + 1 个既有近邻，才进入泛化。防过早抽象。

### ② 变异点识别（不变量 vs 变异）
- 对比"同能力"的多个具体使用，定位**不变的逻辑**(expression×OS log-rank)与**变异的绑定**(study=lihc vs 需要 luad)。
- 技术上：变异点常常就是工具脚本里**硬编码的常量**或**未暴露为参数的 CLI 默认值**(本例 `--study` 默认 lihc)。识别 = LLM 读工具源 + diff 多个使用的实际参数 → 提名"应提升为参数的槽"。

### ③ 泛化合成（提升为参数化通用工具）
- 复用既有 `ToolSynthesizer` 合成机制(AS1-AS8)：以"把 `<变异点>` 从硬编码提升为输入参数、保持其余逻辑不变"为合成指令，产出新版本工具。
- **关键**：这是**重构式合成**(refactor)，输入是既有工具源 + 变异点，不是从零生成——降低幻觉、保留验证血统。

### ④ 验证门（质变必须被证明，不能假设）
- 泛化工具**必须在它声称覆盖的所有案例上重新过 runtime-gate**：至少 `cohort=lihc`(原案例，回归) + `cohort=luad`(新案例，新能力)，都需真实产出非空、字段正确、no-fabrication 通过。
- 任一案例 gate 失败 → **泛化被拒**，回退到"两个特化工具"或合成全新。**这是防止"泛化反而更糟"的核心闸门**——直接复用现有 `validate_candidate_script` / 运行时门。

### ⑤ 扬弃与谱系（Aufhebung）
- 通过 = 注册通用工具为**新版本**(如 `tcga/survival_assoc@0.2.0`，参数化 cohort)，maturity 取决于验证广度。
- 旧特化版本**不删除**，标记为 `superseded_by` / deprecated，但历史证据仍可溯源到产出它的具体版本。
- **双轴本体**：现有 `maturity`(exploratory→wrapped→verified) 之外，新增**特异性轴** `specificity`(specialized→general)。一个工具可从 `exploratory+specialized` 进化到 `verified+general`。
- AS13 的 Methods & Tools 复现段需展示**版本血统**(v0.1 specialized → v0.2 general，及各自验证覆盖)。

### ⑥ 人在环治理（诚实伦理）
- 自动提升会**静默改变**其它在跑假设的工具行为，故泛化提升**surface 成可审决策**(handoff)：
  > "工具 `tcga/survival_assoc` 跨 3 个任务被特化使用(lihc/luad/…)；提议提升为 `survival_assoc(gene,cohort)`，已在 lihc+luad 通过 runtime-gate。是否采纳为通用工具？"
- 默认推荐采纳；人可拒绝/限定范围。证据级工具的提升尤其需要人确认(关联 AS6 verified 客户端的谨慎)。

## 5. B（输出领域校验）作为触发器——最小接口

- **新 LLM seam** `OutputGroundingScorer`(仿 `RelevanceScorer`，Noop 默认=无 LLM 不改行为)：
  `grounds_hypothesis(hypothesis_statement, finding_text) -> Option<bool>`。
- **校验点**：`raise_stance_assessment_for_observation`(`agent.rs:1329`) 之前，读**真实报告内容**(经 `inspect_artifact`/`ArtifactSummary.path`，观察器已有读 artifact 先例)而非仅 observation summary(cohort 在正文不在 summary)。
- `Some(false)` mismatch → 不走正常 stance 交接(那=可用证据)；改为：① 拒绝该发现作为证据；② apply_failure 诚实记录(report 是 lihc，假设是 LUAD)；③ 进入 §3 自纠正分叉(优先进化近邻)。
- `Some(true)`/`None` → 照常(零回归)。

## 6. 护栏汇总（反面审视，逐条待定）

1. **过早泛化** → 三次法则阈值 + 必须"已验证使用"才计数。
2. **泛化引入回归** → 验证门(§④)跨所有 subsumed 案例重跑 runtime-gate；失败即拒绝。
3. **相似性幻觉(错误合并)** → 多重 grounding 合取(I/O 签名 + 语义 + 使用证据)，不单凭 LLM。
4. **语义"形似实异"误合**(survival vs mutation 关联) → 指纹含 `领域动作` 维度；验证门兜底。
5. **谱系/可复现** → 工具版本化 + `superseded_by` + AS13 展示血统；历史证据溯源到具体版本。
6. **静默行为漂移** → 提升走人在环可审 handoff。
7. **确定性边界** → 所有 LLM 判断在 agent/synthesis 层，**`argument.rs` 判决出口保持 0 LLM/网络**。
8. **核心通用性** → 引擎只操作"指纹/变异点/版本"等通用概念，**核心不得写入任何具体基因/疾病/study 常量**；具体内容永远在工件(工具源 + 假设文本)里。

## 7. 与现有原语的映射（可复用，不重造）

| 需求 | 复用现有 |
|---|---|
| 泛化合成 | `ToolSynthesizer`(AS1-AS8) + cBioPortal 客户端(AS6) |
| 验证门 | `validate_candidate_script` / runtime gate(AS4) |
| 版本/谱系 | 工具已版本化(`@x.y.z`)；加 `superseded_by` |
| LLM 判断 seam | `RelevanceScorer` 模式(Noop 默认) |
| 人在环治理 | `DecisionKind` handoff 机制(可能新增 1 个 additive 变体) |
| 复现展示 | AS13 Methods & Tools 段加版本血统 |
| 触发器 B | `raise_stance_assessment_for_observation` 注入点 |

## 8. 不变量（贯穿所有阶段）

- `argument.rs` 判决确定性：0 LLM/网络。
- no-fabrication / 自欺 gate / L4 人在环：保持。
- 核心零单次任务/领域常量：所有具体内容留在工件。
- 非破坏：所有新 LLM seam Noop 默认；新增 enum 变体 additive。

## 9. 分阶段路线（每阶段独立可发、门禁可验，顺序待定）

- **AS15 — B：输出领域校验 + 拒绝**（最小、立即止血"蒙头用"）：OutputGroundingScorer + 读报告内容 + mismatch 拒绝为证据 + 诚实 apply_failure/handoff。**不含进化**，但为它埋好触发点。
- **AS16 — 能力指纹 + 复发检测**（只读、不改工具）：构造指纹、跨任务相似检测、达阈值时**仅 raise 一个"可泛化"建议 handoff**(human-surfaced)，不自动改工具。先让系统"看见"复发。
- **AS17 — 重构式泛化合成 + 验证门**：对采纳的提议，提升变异点为参数、跨案例 runtime-gate；通过则注册新版本。
- **AS18 — 扬弃/谱系 + 双轴本体 + 自纠正分叉接线**：specificity 轴、superseded_by、AS13 血统展示、把 B 的自纠正默认改为"优先进化近邻"。

## 10. 待我们逐条定的护栏（开放问题）

1. 三次法则阈值 N=3 合适吗？"已验证使用"如何精确定义(过了 stance 且 grounded？)。
2. 相似判定的三重 grounding 权重/合取规则。
3. 泛化提升是否一律人确认，还是 exploratory→exploratory 可自动、晋升 verified 才人确认？
4. 变异点识别允许多槽(gene+cohort+profile)还是一次一槽？
5. 验证门要求覆盖"全部历史 subsumed 案例"还是"代表性 K 个"？成本权衡。
6. 失败回退策略：泛化失败时保留几个特化、何时该真合成全新？
7. `OutputGroundingScorer` 是否复用现有 LLM 配置(DeepSeek)还是独立 seam。

---

**本文档是 RFC。** 实现前我们逐条敲定 §10 的护栏；落地严格按 §9 分阶段，每阶段保持 §8 不变量。
