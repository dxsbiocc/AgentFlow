# AS9 实现简报：自动复核 HIGH/MEDIUM/LOW 收口（contained 修复，不含沙箱）

Status: Assigned to Codex（继续 feat/question-aware-fundamental 分支，AS7+AS8+AS8.1+AS8.2 之上新增提交）
Owner: Claude(编排) · Codex(执行)
Spec source: code-reviewer + security-reviewer 对 PR #35 (AS7–AS8.2 diff) 的自动复核结论
Depends on: AS1-AS8.2（本分支已有 5 个提交，core 282 / cli 92 绿，live 验收通过）

## 背景

自动复核结论：0 CRITICAL。本里程碑收口可离线测试、低风险的 4 项（2 HIGH + 1 MEDIUM + 1 LOW）。MEDIUM 中的"运行时出网沙箱 / DNS-rebinding"留作 AS10（独立的 egress allowlist 代理），**不在本简报范围**。

## 编排者裁决（约束）

### 1. [H-CODE] 消除每 cycle 重复的 LLM scorer 调用 —— per-cycle 缓存

`crates/agentflow-core/src/agent.rs`：`enrich_branch_proposal`（~884）与 `has_equivalent_tool_branches`（~1014）在同一 cycle 内对同一 `(tool_ref, hypothesis_statement)` 各自独立调 `apply_semantic_relevance_to_candidates` → 最多 6 次真 LLM `scorer.is_relevant`。成本翻倍，且非确定 scorer 下 `matched_fit` 与 `equivalent_branches` 可能不一致。

裁决：在 `run_cycle_inner`（~354）内，用一个 **per-cycle 记忆化包装器** 包住传入的 `scorer`，再把包装器传入 branch 循环（`enrich_branch_proposal` / `has_equivalent_tool_branches` 两处都用它）。

- 新增私有类型（agent.rs 内，非 pub）：
  ```rust
  struct CachingRelevanceScorer<'a> {
      inner: &'a dyn RelevanceScorer,
      cache: std::cell::RefCell<std::collections::HashMap<(String, String), Option<bool>>>,
  }
  impl<'a> RelevanceScorer for CachingRelevanceScorer<'a> {
      fn is_relevant(&self, hypothesis_statement: &str, tool_ref: &str, tool_description: &str) -> Option<bool> {
          let key = (tool_ref.to_string(), hypothesis_statement.to_string());
          if let Some(cached) = self.cache.borrow().get(&key) { return *cached; }
          let result = self.inner.is_relevant(hypothesis_statement, tool_ref, tool_description);
          self.cache.borrow_mut().insert(key, result);
          result
      }
  }
  ```
  缓存键用 `(tool_ref, hypothesis_statement)`（与 prompt 决定性输入一致；`tool_description` 由 tool_ref 唯一决定，不入键）。
- 在 `run_cycle_inner` 进入 branch 循环前构造一次 `let cycle_scorer = CachingRelevanceScorer { inner: scorer, cache: Default::default() };`，循环内把原来传 `scorer` 的地方改为 `&cycle_scorer`。单线程 cycle，`RefCell` 安全。
- **更新受影响测试**：`keyword_medium_demoted_to_low_triggers_auto_synth_gap`（agent.rs ~4829 附近）此前断言 `scorer.calls()` 返回该 tool_ref **两次**；缓存后应为 **一次**——把断言改为单次。其余断言（fit=low、demoted reason、synthesizer 调用、auto-synth skipped）保持。其它直接调 `has_equivalent_tool_branches(..., &scorer)` 的 AS8.2 单测仍直接用裸 stub scorer（不经缓存包装），保持不变。
- 新增一个单测证明缓存生效：构造一个会记录调用次数的 stub，跑一个 `run_cycle_with_synth` cycle，断言对同一 (tool, hypothesis) `is_relevant` 只被调用一次（而非 enrich + has_equivalent 两次）。

### 2. [H-SEC] probe fetch 禁止跟随重定向（防 allowlist 绕过 → SSRF）

`crates/agentflow-cli/src/synth_commands.rs`：`SOURCE_PROBE_FETCH_PY`（51-65）与 `CBIOPORTAL_DISCOVERY_FETCH_PY`（39-50）用 `urllib.request.urlopen`，默认跟随 301/302/307/308 重定向且不重新校验目标 host。allowlisted 站点的 open-redirect 可把 probe 重定向到 `http://169.254.169.254/`（云元数据）或 `http://127.0.0.1:<port>/`。

裁决：两个 PY 常量都改为 **禁止重定向** 的 opener：
```python
class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None  # never follow redirects across hosts

_opener = urllib.request.build_opener(_NoRedirect)
with _opener.open(request, timeout=timeout) as response:
    ...
```
- 非重定向响应行为完全不变（正常 2xx 仍读 body）。
- 服务器返回 3xx 时，`build_opener(_NoRedirect).open()` 会因 `redirect_request` 返回 `None` 而抛 `urllib.error.HTTPError`（3xx 当错误处理）→ `fetch_probe`/调用方走既有"failed"分支，trace 记 probe 失败。这是期望行为（拒绝盲目跟随）。
- 不改 argv 约定、不改 `MAX_SOURCE_PROBE_BYTES`/timeout。
- 注释说明这是 SSRF 防护（拒绝重定向以免绕过 host allowlist）。

### 3. [M-probe-cap] 限制单次源发现的网络探测数量

`crates/agentflow-cli/src/synth_commands.rs` 源发现循环（~646 `for candidate in &candidates`）无上限，LLM 多提议时串行阻塞数分钟。

裁决：新增 `const MAX_PROBED_SOURCE_CANDIDATES: usize = 5;`。**通过 host allowlist 安全检查、即将发起真实网络 probe** 的候选数量封顶为该值；达到上限后，剩余候选不再发起网络请求，trace 追加一行 `- <label>: skipped (probe budget MAX_PROBED_SOURCE_CANDIDATES reached)`。被 `source_probe_safety` 直接拒绝的候选（根本不联网）不计入预算。`candidate proposals parsed` 的计数行保持原值（反映 LLM 提议总数）。

### 4. [LOW] 清理 `has_pending_source_discovery_gap` 死分支

`crates/agentflow-core/src/agent.rs` ~865：filter 同时匹配 `FundamentalGap || DeepenOrStop`，但 `SOURCE_DISCOVERY_GAP_HYPOTHESIS_MARKER`（`source_gap_hypothesis_id = `）只由 `auto_synth_research_gap_digest`（仅用于 `FundamentalGap`）写入；`DeepenOrStop` 的 `strong_verdict_digest`/`graph_patch_apply_digest` 从不含该 marker，故 `source_discovery_gap_hypothesis_id` 对 DeepenOrStop 恒返回 `None`——该半边是死代码。

裁决：移除 `|| point.kind == DecisionKind::DeepenOrStop`，只保留 `point.kind == DecisionKind::FundamentalGap`。AS8 既有的 FundamentalGap 去重单测（以及 live 第二轮 `raised_decisions=[]`）保持绿即证明去重不回归。若无直接覆盖该函数的单测，新增一个最小单测：同一 hypothesis 已有 pending FundamentalGap 时 `has_pending_source_discovery_gap` 返回 true，无时返回 false。

## 不在本里程碑（→ AS10）

- M1 DNS-rebinding/TOCTOU、M3 合成工具运行时出网管控：留给 AS10 的 egress allowlist 本地代理（probe 与 runtime 统一走代理，在 CONNECT 层校验 host + 私网 IP），届时在网络层兜底重定向/rebinding。
- 不改 `argument.rs` 判决逻辑、不改 allowlist 内容、不改 `DecisionKind`/事件结构。

## 交付物

- `agent.rs`：`CachingRelevanceScorer` + `run_cycle_inner` 接线；`has_pending_source_discovery_gap` 去死分支；缓存单测 + FundamentalGap 去重单测（如缺）。
- `synth_commands.rs`：两个 FETCH_PY 禁重定向；`MAX_PROBED_SOURCE_CANDIDATES` 探测预算 + trace。
- 受影响测试断言更新（`keyword_medium_demoted_to_low_triggers_auto_synth_gap` 改为单次 scorer 调用）。

## 验收标准（Claude 复核）

- [ ] fmt / clippy / `cargo test -p agentflow-core` / `cargo test -p agentflow-cli` / `scripts/acceptance-v1.sh` 全绿。
- [ ] `argument.rs` 仍 0 处 LLM/网络调用。
- [ ] 单测证明 per-cycle 缓存：同一 (tool, hypothesis) `is_relevant` 每 cycle 仅 1 次。
- [ ] 两个 FETCH_PY 含 `_NoRedirect` opener；非重定向响应行为不变（既有 runtime-gate 测试保持绿）。
- [ ] 探测预算生效（可加一个解析多候选、断言 trace 出现 "probe budget ... reached" 的单测；若联网难测则至少断言常量与 trace 文案存在）。
- [ ] `has_pending_source_discovery_gap` 仅匹配 FundamentalGap，去重不回归。
- [ ] core 测试数不减少；无新依赖。
