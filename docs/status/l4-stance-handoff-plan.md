# L4 实现简报：auto-run 后 raise stance 决策点（修「结果惰性」核心缺陷）

Status: Implemented + verified (2026-06-03)
Date: 2026-06-03
Owner(orchestrator): Claude · Executor: Codex
Spec source: 深度测试审计发现的 🔴 核心缺陷 —— 自主分析结果被静默存 Neutral、判决永不前进、解读未交人类
Depends on: L3（auto_run_applied_step）、H3（决策点）、S2（封顶）—— 均已合并 main

## 验收记录（Claude 独立复验 2026-06-03）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **192**（基线 191，+1）/ cli 61 / schemas 3 全绿。
- ✅ 改动局限 `agent.rs`（auto-run 路径）/ `handoff.rs`（+StanceAssessment）/ `agent run` CLI；**`argument.rs` 改动 0 行**（judge/cap/封顶未动）；Cargo 零变更；无新表/event_type/依赖。
- ✅ 生产 auto-run 路径不再自动 `link_evidence(Neutral)`；`Stance::Supports` 仅出现在测试代码。
- ✅ 非 auto-run 路径零回归；L3 auto-run 测试更新为断言 StanceAssessment。
- ✅ **全自主链 live 验证（真 TCGA + 真 claude）**：auto-run 跑出观察 → `outcome=handed_off`、**无自动 Neutral 入账**、raise `stance_assessment` 决策，digest **含真实发现「score -1.165」+ 假设 + observation_id + evidence link 操作建议**，options=[supports/contradicts/inconclusive]，不自动判 stance。
- ✅ **闭环 live**：人类据发现 link supports → 证据 `[inferred/supports]` 进入论证 → 报告 supporting +1、判决反映；但 exploratory 封顶→仍 `inconclusive(provisional)`，**不独断 affirmed**（防自欺 + §15 完好）。

结论：合并就绪。审计 🔴「结果惰性 / 解读未交人类」核心缺陷修复——自主收集 + 人类 gated 解读 + 证据进论证 + 判决诚实不独断，闭合「自主科研做得诚实」。LLM 提议 stance（capped，人类确认）= L5 后续。

## 目标（修审计 🔴 + A2/A3 缺口）

把 L3 的「auto-run 后静默 link Neutral」改为「**raise 一个 `StanceAssessment` 决策点，digest 嵌入真实发现**（观察 summary，如 `THRSP score -1.165`），请人类判定它对假设支持/反对/无法判定」。这样：
- 自主分析结果**不再惰性**——显式「待解读」而非被忽略。
- 关键解读判断**交回人类**（A2/A3），且 digest 里**带上真实发现**（顺带修审计 🟠 报告藏发现：发现会出现在报告 Open Decisions 段）。
- 人类据 digest 用 `evidence link --observation <id> --stance supports|contradicts` 把证据真正接进论证 → 判决能动。

## 编排者裁决（约束）

1. **不再自动 link Neutral**：auto-run 跑出观察后，改为 raise `StanceAssessment` 决策点（避免「Neutral + 人类后续 link」双计数）。观察仍存在、被决策点引用。
2. **不自动判 stance**：L4 只 surface 解读决策，绝不让系统/LLM 自动判支持/反对（LLM 提议 stance、capped → L5 后续）。
3. **判决/封顶逻辑不动**；改动局限 `agent.rs`（auto-run 路径）+ `handoff.rs`（DecisionKind）。
4. 仅 `--auto-run` 路径行为变化（L3 的 auto-run 测试相应更新，语义演进）；非 auto-run 路径零变化；现有 191 core / 61 cli 既有非-auto-run 断言不改。
5. 不新增依赖/表；event_type 沿用 `handoff.decision_point_raised`。

## 交付物

### 1. `handoff.rs`
- `DecisionKind` 增 `StanceAssessment`（+ as_str=`"stance_assessment"` / parse）。

### 2. `agent.rs` —— `auto_run_applied_step`
- 改签名：增 `raised_decisions: &mut Vec<DecisionPoint>`（caller 传循环的 raised_decisions）。
- 跑出 `observation_id` 后，**删除原 `link_evidence(Neutral)` 块**，改为：
  - `obs = inspect_observation(observation_id)` 取 `obs.summary`（真实发现）。
  - `statement = inspect_hypothesis(hypothesis_id).statement`。
  - **dedup**：若 `pending_decision_points()` 已有针对该 `observation_id` 的 StanceAssessment（用 digest 内嵌的 observation_id 标记判断），跳过。
  - 否则 `raise_decision_point(DecisionKind::StanceAssessment, digest, options, recommendation=2)`：
    - digest：`分析步骤 <step_id> 产出真实发现：<obs.summary>。请判定它对假设「<statement>」的立场。若支持/反对，运行：evidence link --hypothesis <hypothesis_id> --observation <observation_id> --stance supports|contradicts --grade observed`（含 observation_id 作 dedup 标记）。
    - options：`["supports — 该发现支持假设", "contradicts — 反对假设", "inconclusive — 暂无法判定/需更多证据"]`，recommendation=2（保守默认，不诱导）。
  - 把决策点 push 进 `raised_decisions`。
- `StepRun` action 仍 push。run 失败路径不变（记 apply_failures）。

## 验收标准（Claude 审核逐条核对）

- [ ] **回归**：非 `--auto-run` 路径行为零变化；现有非-auto-run 测试不改且通过；L3 的 auto-run 测试更新为断言「raise StanceAssessment（不再自动 link Neutral）」，语义清晰。
- [ ] `clippy -D warnings` 无警告；`cargo test` 净增、全绿；无新依赖/表/新 event_type。
- [ ] 行为测试（离线，用本地 exploratory 工具）：`agent run --apply --flow --auto-run` → 跑出观察 → raise 一个 `StanceAssessment` 决策点，digest **含观察 summary（真实发现）+ 假设 statement + observation_id**；**不再有 Neutral 证据自动入账**；outcome=handed_off。
- [ ] dedup：同一 observation 的 StanceAssessment 不重复 raise。
- [ ] grep 确认 judge/cap/封顶逻辑未改；auto-run 路径不自动判 stance（无自动 supports/contradicts link）。

## 不在本里程碑（明确排除）

- LLM 自动提议 stance（capped，人类确认）→ L5。
- 决策 resolve 自动触发 evidence re-link（当前人类用 `evidence link` 手动接）→ 后续可加。
- 报告专门渲染观察 finding（现经决策 digest 已部分体现）、YAML 健壮性、参数有效性校验 → 各自独立后续。
