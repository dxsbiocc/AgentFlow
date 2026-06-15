# Plan A 实现简报：干净本地生存工具（证明成功路径用的标本）

Status: Assigned to Codex
Owner: Claude(编排) · Codex(执行)
目的: 给"成功路径端到端验证"造一个**快、本地、离线、真计算**的标本工具——不碰 cBioPortal(已证是差标本)。数据已由编排者一次性下载到 `examples/data/lihc_demo/`(真实 TCGA-LIHC,365 患者,SPP1 高表达→更差 OS,log-rank p≈0.00004)。

## 交付物

### 1. `examples/tools/local_survival_assoc.py`（离线、stdlib、快）

- 等价于 `examples/tools/tcga_survival_assoc.py` 的**统计部分**(median 切分 + 两组 log-rank),但**不联网**:从**本地 TSV**读数据。复用其 `median` / `logrank` / `median_survival` 数学(可直接照搬同样实现,stdlib only)。
- 输入(AgentFlow 工具 env 约定,参考 marker_survival_scan.sh 的 `AGENTFLOW_INPUT_*` / `AGENTFLOW_PARAM_*` / `AGENTFLOW_OUTPUT_*`):
  - `AGENTFLOW_INPUT_EXPRESSION_TABLE`:TSV,首行 `sample\t<GENE1>\t<GENE2>...`;每行一个 sample 的表达值。
  - `AGENTFLOW_INPUT_SURVIVAL_TABLE`:TSV,首行 `sample\ttime\tstatus`(status 1=event/death,0=censored)。
  - `AGENTFLOW_PARAM_GENE`:基因符号,必须是 expression 表里的一个列名;缺列→`SystemExit` 报错(不可 fabricate)。
  - `AGENTFLOW_OUTPUT_REPORT`:输出报告路径。
- 逻辑:按 sample join 两表 → 取 gene 列表达值 → median 切分 high/low → 对应 survival 跑 log-rank → 算 signed `score = sign * -log10(p)`(sign:high 组 median OS 更短为 +,即"高表达更差")。
- 输出 **marker_report 格式**(与 tcga_survival_assoc 一致,让 `marker_report` observer 能解析):至少包含
  ```
  Marker report
  Gene: <gene>
  score: <signed -log10(p)>
  n: <joined samples>  high: <n_hi>  low: <n_lo>
  events: high=<..> low=<..>
  median_OS: high=<..> low=<..>
  logrank_chi2: <..>
  logrank_p: <..>
  direction: <high-expression associated with worse|better overall survival>
  source: local imported expression+survival cohort
  ```
  诚实:只报真实算出的数,不编造;样本太少(如 join<6)→`SystemExit`。

### 2. `examples/tools/local_survival_assoc.tool.yaml`

```
schema_version: agentflow.tool.v0
namespace: local
name: survival_assoc
version: 0.1.0
maturity: wrapped
description: Local survival association for a gene over imported expression and survival tables (median split, log-rank). Offline, no network.
validator_profile: paired_expression_survival_v0
inputs:
  expression_table:
    required: true
    required_columns: sample
  survival_table:
    required: true
params:
  gene:
    type: string
    required: true
outputs:
  report:
    type: Markdown
    observer: marker_report
    min_rows: 3
runtime:
  backend: local
  command:
    - /usr/bin/env
    - python3
    - local_survival_assoc.py
```
（command 第三项相对路径,运行时按既有 local 工具约定从工具目录解析;参考 marker_survival_scan.tool.yaml 用相对脚本名。）

### 3. 离线测试

- 在 examples 或 cli 测试里,用一个**小 fixture TSV**(自带 ~8-10 行,带明显信号:高表达组 time 更短/event 更多)跑该脚本,断言:输出含 `Gene:`、`logrank_p:`、`direction:`,且 direction 与 fixture 信号一致;缺 gene 列→非零退出;join 太少→非零退出。
- 不联网(纯本地 TSV)。

## 约束

- 不改 core / argument.rs / 既有 examples 工件(只**新增** local_survival_assoc.{py,tool.yaml} 与测试)。
- stdlib only,无新依赖,离线。
- 通用(任意 gene/任意导入数据),无单次任务/疾病常量写死(cohort 来自导入数据,gene 来自参数)。

## 验收（编排者后续 live）

- 注册该工具 + import `examples/data/lihc_demo/{expression,survival}.tsv` + 建假设"High SPP1 expression is associated with worse overall survival in TCGA-LIHC hepatocellular carcinoma" → 纯 `agent run` → 期望:匹配 local/survival_assoc → 真跑 → 真实发现(SPP1 高→更差,p≈0.00004)→ output-grounding **通过**(领域匹配,不被拒)→ raise stance(带真实发现)。到 affirmed 由人 resolve stance(设计如此)。
