# S3 实现简报：循环遇能力缺口 → raise 合成决策点（gated）

Status: Implemented + verified (2026-06-03)
Date: 2026-06-03
Owner(orchestrator): Claude · Executor: Codex
Spec source: §15 Tool Gap Resolution 的「capability_needed → decision → user_approval」+ 回顾发现的 S1 未尽部分
Depends on: S1（synth）、S2（grade 封顶，安全前提）、H3（决策点）、T2（EnrichedProposal）—— 均已合并 main

## 验收记录（Claude 独立复验 2026-06-03）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **186**（基线 182，+4）/ cli **54**（+1）/ schemas 3 全绿。
- ✅ 改动局限 `handoff.rs`（+ToolGap）/ `agent.rs`（ApplyConfig.propose_synth + raise）/ `agent run` flag；Cargo 零变更；无新表/event_type/依赖。
- ✅ **默认关零回归**：不传 `--propose-synth` → 不 raise（live：Raised 0 / advanced）；现有测试未改且通过。
- ✅ **ToolGap 路径只 raise**：生产路径仅 `raise_decision_point`；`register_tool`/`fs::write` 均在 `#[cfg(test)]`（line 838 之后）；无自动合成。
- ✅ dedup：同假设已有 pending ToolGap 不重复 raise（测试覆盖）。
- ✅ **live**：`--propose-synth` → raise `[tool_gap]`，digest 含「能力需求=<statement>、无工具匹配、建议 `agentflow synth ... --fixture <...> --expect <...>`、需人类批准+fixture」。有匹配工具则不 raise。

结论：合并就绪。循环现可**自主识别能力缺口 → 提议合成（gated，带 §15 决策痕迹）**，但绝不盲目自动写代码。补上回顾发现的 §15「decision-trail + user_approval」缺口。

## 目标

让自治循环在**遇到能力缺口**（Deepen/Spawn 提议无匹配工具）时，**自主识别并 raise 一个「合成工具」决策点**交人类（gated），而**不是**盲目自动写代码。决策 digest 记录 §15 要求的决策痕迹。这把「缺工具时自主写代码」honestly 接进循环：循环**提议**合成，人类提供 fixture + 批准，再用 `agentflow synth` 落地（S1）。S2 的 grade 封顶保证即便合成出来，其证据也低信任。

## 编排者裁决（约束）

1. **opt-in 默认关**：`agent run --propose-synth`（默认 false）。不传时行为与现状逐字节相同，现有 182 core / 53 cli 测试不改且通过（首要回归）。
2. **不盲目合成**：循环**只 raise 决策点**（A2），绝不在循环内自动调用 synth/写代码/注册工具。合成仍是用户 gated 的 `synth` 命令（需 fixture）。
3. **dedup 防刷屏**：同一 hypothesis 已有 pending `ToolGap` 决策时，本轮不再重复 raise。
4. 改动局限 `handoff.rs`（加 DecisionKind）+ `agent.rs`（config + raise）+ `agent run` CLI（flag）。不新增依赖/表；event_type 沿用 `handoff.decision_point_raised`。
5. 质量门全绿；基线 core 182 / cli 53 / schemas 3。

## 交付物

### 1. `handoff.rs`
- `DecisionKind` 增 `ToolGap`（+ `as_str`/`parse`，模式同其余取值）。

### 2. `agent.rs`
- `ApplyConfig` 增字段 `propose_synth: bool`（`Default` = false；现有构造处补默认）。
- `run_cycle` 在 Deepen/Spawn 处理处：当 `config.propose_synth && proposal.matched_tool.is_none()`：
  - **dedup**：若 `pending_decision_points()` 中已有针对该 `hypothesis_id` 的 `ToolGap`，跳过。
  - 否则 `raise_decision_point(DecisionKind::ToolGap, digest, options, recommendation=0)`，并入 `raised_decisions`。
  - **digest（§15 决策痕迹）**：含「能力需求 = <hypothesis statement>」「现状 = 无注册工具匹配（registry_match 失败）」「建议 = 合成一个 exploratory 工具：`agentflow synth --name <…> --description "<…>" --fixture <你的已知答案文件> --expect <…>`」「需人类批准 + 提供验证 fixture」。
  - **options**：`["合成一个工具（提供 fixture 后 synth）", "注册一个已有工具", "跳过该分支"]`，recommendation = 0。
- `CycleReport` / `agent run` 输出无需新结构（ToolGap 决策走既有 `raised_decisions`）。

### 3. `agent run` CLI
- 加 `--propose-synth`（bool，默认 false）→ `ApplyConfig.propose_synth`。usage 追加。

## 验收标准（Claude 审核逐条核对）

- [ ] **回归**：不传 `--propose-synth` 时行为零变化；现有 182 core / 53 cli 测试不改且通过（首要）。
- [ ] `clippy -D warnings` 无警告；`cargo test` 净增、全绿。
- [ ] 无新依赖/表/新 event_type。
- [ ] 行为测试：①`--propose-synth` 下，一个无匹配工具的 Deepen 提议 → raise 一个 `ToolGap` 决策点（digest 含能力需求 + synth 建议）；②不传 flag → 不 raise（回归）；③dedup：同假设第二轮不重复 raise（已有 pending ToolGap）；④有匹配工具的提议 → 不 raise ToolGap。
- [ ] grep 确认：`agent.rs` 在 ToolGap 路径**不**调用 synth/register_tool/写文件——只 raise 决策点。
- [ ] `DecisionKind::ToolGap` 的 as_str/parse 往返测试。

## 不在本里程碑（明确排除）

循环内自动执行 synth（需自动生成 fixture = 自我验证陷阱，刻意不做）、LLM 接进 ArgumentEngine/BranchSelector、exploratory→verified 晋升流水线、真沙箱。
