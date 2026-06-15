# 简报：affirmed 成功路径回归基准（通用、合成、零真实数据/LLM/网络）

Status: Assigned to Codex
目的: 锁住"成功路径**机制**"不被悄悄破坏——**不是**重放 SPP1/LIHC 那个任务,而是用**合成 fixture** 断言"verified 工具 + 确认参数 + observed 证据 → affirmed",以及"未验证/推断参数 → 被 cap → 上不去 affirmed"这些诚实门。无任何 SPP1/疾病/真实数据/LLM/网络。

## 背景

整 session 只 live 证过失败/交接路径;Plan A 手动证了成功路径到 affirmed,但没有**自动回归测试**守着它。本测试把成功路径的**通用机制**固化:
- affirmation 规则(argument.rs:587):`margin>=3 && has_obs_support` → Affirmed。
- grade-cap(argument.rs:687):`observed` 在 (源工具非 verified) 或 (源步骤有推断参数) 时被降到 `inferred`。
这些已有零散单测;本测试补一个**端到端**集成断言把整条链串起来。

## 交付物:`crates/agentflow-cli/tests/success_path_affirm.rs`(新增集成测试)

用**合成**工件,参考既有 `crates/agentflow-cli/tests/local_survival_assoc.rs` 与 `scripts/acceptance-v1.sh` 的 CLI 用法(init / tools register / import / flow validate+approve / run / observe / evidence link / verdict render / verdict show / hypothesis show)。**不联网、不调 LLM**(纯本地工具 + 显式参数,不走自治 agent run 的推断)。

合成 fixture(测试内临时生成,无领域语义):
- 一个**最小本地工具** `marker_emit`(临时 `.sh` 或复用一个测试内写出的脚本):读两个输入表 + 一个 param `marker`,emit 一个 **marker_report 格式**报告(固定合成数:`Marker report\nGene: <marker>\nscore: 0.9\n...` 至少 min_rows 行,含 marker_report observer 能解析的字段)。namespace/name 任意(如 `test/marker_emit`)。
- 两个最小 TSV 工件(`sample\tM1` 表达表、`sample\ttime\tstatus` 生存表,几行即可),import 时 `--type ExpressionTable` / `--type SurvivalTable`,工具 inputs 用相同 type。

测试用例:

**1. 正路径 → affirmed**
- 注册该工具,maturity = **verified**;import 两表;建假设(任意陈述,如 "marker M1 associates with outcome in the imported cohort")。
- 构造 flow,step 用该工具,inputs=两 artifact id,**param marker=M1 显式**(确认参数,非推断);validate+approve+run → 得 observation。
- `evidence link --stance supports --grade observed`(同一 observation);断言 `evidence list` 该证据 grade 仍是 **observed**(verified 工具 + 无推断参数 → 不被 cap)。
- `verdict render`(填齐 self-deception gate 各 `--gate-*`,`--gate-claim-basis observed`);断言 **verdict show / render 输出 verdict = affirmed**。

**2. 负路径 A:未验证工具 → 被 cap → 上不去 affirmed**
- 同样流程,但工具 maturity = **exploratory**(或 wrapped);link observed → 断言证据 grade 被降为 **inferred**;render verdict → 断言 verdict **不是 affirmed**(应为 inconclusive/provisional)。

**3. 负路径 B(可选):推断参数 → 被 cap**
- 若便于构造:一个带"该 observation 步骤有推断参数"标记的场景 → observed 被降 inferred → 非 affirmed。(若集成层不易构造推断参数标记,可跳过此例,核心已由 argument.rs 单测覆盖;在测试注释里说明。)

## 约束

- **通用、合成**:测试内 fixture 无任何 SPP1/基因/疾病/真实 study 常量;不读 `examples/data/lihc_demo/`;不依赖真实 cBioPortal/网络/LLM。
- 不改 core / argument.rs / 既有工件;只**新增**该集成测试(+ 测试内临时文件用 tempdir)。
- 断言的是**机制/规则**(affirmed 可达且被诚实门正确把守),不是某个具体任务的数值。
- 无新依赖。

## 验收

- [ ] `cargo test -p agentflow-cli --test success_path_affirm` 全绿,覆盖正路径 affirmed + 负路径 cap→非 affirmed。
- [ ] `cargo test --workspace` / fmt / clippy 全绿;core 未改。
- [ ] 测试无 SPP1/疾病/真实数据/LLM/网络依赖(可离线在 CI 跑)。
