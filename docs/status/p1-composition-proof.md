# 验证记录：P1 隔离执行 + 组合在真实工具上的端到端证明

Date: 2026-06-22
Status: PASS — 组合 + per-step I/O staging + 计算工件谱系在真实本地工具上端到端跑通,且诚实性不变量两路均生效。

## 动机(初心回顾驱动)

P1.1–P1.4 把隔离执行引擎建了出来,但 P1.2 的隔离 env 只用 fake provisioner 测过、组合从未在真实多工具流上验证——"机制完成、live 未证",与工具进化引擎当初同样的风险。为防"加层快于验证"的漂移,先用一个真实两步组合流验证 P1 交付价值,再决定是否下探 container/调度。

## 交付物(example 工件,非 core)

- `examples/tools/expression_select.{py,tool.yaml}` —— 一个确定性、离线的**生产者**工具:对 ExpressionTable 按 `genes` 参数选列,产出更小的 ExpressionTable(`maturity: wrapped`)。
- `examples/flows/composed_demo.flow.yaml` —— 两步组合流:`select`(生产者)→ `assoc`(`local/survival_assoc` 消费者),step2 的 `expression_table` 输入引用 `select.selected`(producer.output)。

## 实跑结果(AgentFlow 自行执行,我只分析)

### 1. 玩具数据(4 样本)—— 诚实拒绝
`run composed_demo`:`select` succeeded、`assoc` **failed**,stderr:`too few joined samples (4) for TP53; need at least 6`。
→ 这不是 bug,是 **no-fabrication 不变量**:样本不足时工具诚实失败,不编造 log-rank。

### 2. 真实 TCGA-LIHC 切片(365 样本)—— 端到端成功
`run composed_demo`:**Completed steps: 2, Failed steps: 0**。
- `assoc` 的 `inputs.json`:`expression_table` = `<assoc-workdir>/inputs/expression_table/selected` —— 即 **生产者 step 的输出被 stage 进消费者 workdir**(P1.3 验证)。
- marker_report(真实结果,非显著):`Gene: TP53, n=365 (high 183/low 182), logrank_p=0.741, direction: high→worse OS`。诚实的非显著结论。

### 3. 计算工件谱系(可审计)
`artifacts list`:
- imported ExpressionTable / SurvivalTable(source_step=None)
- **computed** ExpressionTable,source_step=`composed_demo/select`
- **computed** Markdown,source_step=`composed_demo/assoc`
→ 完整链:导入表 → select 计算产出 → assoc 计算产出。组合只经声明 I/O,谱系全程可追溯。

## 结论

- **愿景 #3(灵活组合)+ #4(I/O 作为标准接口)+ P1.3 staging:在真实工具上 live 证明。**
- **初心(诚实闭环)守住**:玩具数据→诚实拒绝;真实数据→诚实非显著 + 全谱系。
- 课题方向无漂移:隔离执行引擎确实服务"可复现、可组合、诚实的研究",不是空转基建。

## 后续(已记为 follow-up)

- 把本组合 + staging 行为锁成回归集成测试(仿 `success_path_affirm.rs`,合成 shell 工具,离线无 python 依赖),防止未来回归。
- 据此再决定 P2:container 后端(硬隔离,关 #36)vs agent 智能调度。
