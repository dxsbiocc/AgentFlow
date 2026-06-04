# ②c 实现简报：serde_json 第二批（argument / branch / agent）

Status: Assigned to Codex
Owner: Claude(编排) · Codex(执行)
Depends on: ②a/②b（serde_json 样板，已合并 main）

## 目标
按 ②a/②b 样板，把 argument.rs / branch.rs / agent.rs 手写 JSON 换 serde_json、删本地重复助手，byte-identical + 旧 payload 兼容。

## ⚠️ argument.rs 的关键难点（必须按此处理，否则破坏 byte-identical）
现有 JSON 对 `Verdict`（带数据枚举）做了**扁平化**：`verdict_payload_text` 产出 `"affirmed"/"refuted"/"inconclusive_provisional"/"inconclusive_fundamental"` 作为 **flat 字符串**，VerdictReport.to_json / verdict_rendered payload 用的就是这个扁平形状。
- **绝不能**对 `Verdict` enum 直接 `#[derive(Serialize)]`（会变成 `{"Inconclusive":{"Provisional":...}}` 嵌套，破坏 JSON）。
- 正确做法：payload/report 的 serde 结构体用 **String 字段**（如 `verdict: String`、`grade: String`、`stance: String`、`inconclusive_kind`/`frontier`/`missing` 等按现有 JSON 的确切 key 与形状），用现有 `as_str()`/`verdict_payload_text()`/`parse` 在 enum↔string 间转换。即：serde 负责结构体 ↔ JSON，enum↔string 仍走现有映射函数。
- EvidenceGrade/Stance/ClaimBasis/VerdictTag 这些简单枚举可 derive+rename 使序列化==as_str（observed/inferred/literature_supported/hypothesis/unsupported / supports/contradicts/neutral 等）。

## 样板（②a/②b）
payload 同名同序结构体 derive；to_json→serde_json::to_string(self)，字段声明顺序=现有 key 顺序；删本地助手；旧 payload 仍能 from_str。

## 约束
1) byte-identical / 现有测试不改且通过（首要，argument 尤其）；旧手写 payload 仍可读。
2) 领域结构体（Verdict/EvidenceLink/VerdictReport/BranchDecision/CycleReport/EnrichedProposal/AppliedAction 等）公开 API 不变；枚举 as_str/parse/逻辑不变（只加 derive+rename，且不改变扁平化语义）。
3) 仅改 argument.rs/branch.rs/agent.rs（+枚举 derive）；不动其它模块；不新增依赖（serde_json 已在）。
4) 删三模块本地重复 JSON 助手（被批外共用的保留并确认）。

## 验收
- [ ] clippy -D warnings 干净；cargo test 全绿；acceptance-v1.sh 通过。
- [ ] 三模块现有测试不改且通过；各加 exact-byte to_json 测试 + 旧 payload 兼容测试。
- [ ] **argument：verdict_rendered payload 与 VerdictReport::to_json 逐字节不变**（含 inconclusive_provisional/fundamental 扁平字符串、supporting/contradicting 列表形状）。
- [ ] 三模块本地 JSON 助手清零。
- [ ] 改动仅这三个 .rs。

## 不在本里程碑
comparison/report/observer/graph_patch（下一批）、clap（③）。

## 验收记录（Claude 独立复验 2026-06-04）
- ✅ clippy 干净；cargo test core 216(+7)/cli 61/schemas 3 全绿；acceptance 通过。
- ✅ 改动仅 argument/branch/agent（净删：argument 522/342、branch 335/138、agent 263/209）；三模块本地 JSON 助手清零。
- ✅ **verdict JSON 逐字节对照基线一致**（Claude 独立复验，归一化时间戳后）——扁平 Verdict（affirmed/inconclusive_provisional + inconclusive_kind/missing/frontier + supporting/contradicting 列表）byte-identical 保持。
- ✅ 旧手写 payload 兼容；现有测试不改通过；无新依赖。
结论：合并就绪。剩余 comparison/report/observer/graph_patch（②d）+ clap（③）。
