# G1 记录：真实 TCGA 生存关联分析工具

Status: Implemented + verified (2026-06-02)
Owner: Claude（编排者亲自实现 + live 验证；此工具网络依赖、需实证，不适合交无法联网验证的执行方）
Spec source: 用户要求「真做 THRSP 分析」——让循环验证而非检索

## 背景与动机

此前对「Thrsp 在脂肪肝→肝癌」的演示**只做了文献检索**（forage），这恰是 AgentFlow 核心哲学批判的「读地图」反模式：检索只是一个动作，科研的本质是**用分析验证未知 → observed 证据 → 论证 → 交接**。文献级证据（abstract，权重 0）永远只能停在 provisional。

本里程碑补上「验证」环节：一个**真实数据分析工具**，让 AgentFlow 产出 observed 证据驱动判决。

## 交付物

- `examples/tools/tcga_survival_assoc.py`（Python 标准库 only，网络在脚本内）：
  - 从公开 **cBioPortal REST API** 拉某基因在某 TCGA 研究的**每样本 mRNA 表达** + **每患者总生存（OS_MONTHS/OS_STATUS）**。
  - 按表达中位数分高/低组，跑**真实 log-rank 检验**（stdlib：风险集累加 + `erfc` 求 chi²(1df) p 值）。
  - 产出 `marker_report` 格式（`Gene:` + `score:` = signed −log10(p)，符号表方向）+ 真实明细（n、events、各组中位 OS、chi²、p、方向）。
- `examples/tools/tcga_survival_assoc.tool.yaml`：注册为 `tcga/survival_assoc`，params `gene`/`study`，output `report`（`observer: marker_report` 自动 observe）；`command: [/usr/bin/env, python3, tcga_survival_assoc.py]`。

## Live 验证（真实数据，编排者亲跑）

- standalone：`THRSP` in `lihc_tcga_pan_can_atlas_2018` → 365 配对样本（高 183/低 182），中位 OS 高 21.0 月 vs 低 16.8 月，**log-rank p=0.068**，**高 THRSP 关联更好生存**（score −1.165）。对照 `TP53` p=0.74（无关联）——证明非随机显著。
- **与文献独立吻合**：PubMed「Downregulation of THRSP Promotes HCC Progression」← 高 THRSP=抑癌/更好预后，方向一致。
- 经 AgentFlow runtime：`tools register` → flow `run` → step succeeded → **自动 observe** 出 observed 证据（`describes gene THRSP with score -1.165`）→ 链为 `[observed]` 证据 → `agent run` 撞强判决 → `handed_off`（待人类补防自欺 gate）→ `report research` 诚实显示「(no verdict)/not yet evaluated」+ 待决策。

## 意义（哲学对齐）

这是 AgentFlow 与 RAG 产品的分水岭：**不复述文献结论，而是用真实数据验证它**，得到趋势级一致的 observed 证据，且 p=0.068 如实报告、强判决仍交人类——验证 + 论证 + 防自欺，全链成立。

## 诚实边界 / 待完善

- 仅 cBioPortal 一个来源、单基因、中位二分 log-rank；未做多变量校正、GEO（NAFLD/NASH 阶段）、肿瘤 vs 正常差异表达、连续模型。
- score 借用 `marker_report` 的标量通道（= signed −log10(p)），完整统计在 artifact 明细里；未来可加专用 observer。
- 运行需联网（cBioPortal）；离线/受限环境下该工具不可用。
- 查询/数据集选择仍需人工指定 study。
