# H5 实现简报：防自欺强制闸门

Status: Implemented + verified (2026-05-31)
Date: 2026-05-31
Owner(orchestrator): Claude · Executor: Codex
Spec source: [`agentflow-agent-control-layer-design.md`](../agentflow-agent-control-layer-design.md) §4.5 / §9-H5
Depends on: H1（argument.rs，已验收）

## 验收记录（Claude 独立复验 2026-05-31）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **141**（基线 135，+6）/ cli 22 / schemas 3 全绿。
- ✅ Cargo 零变更；无新表；无新 event_type（`argument.verdict_rendered` payload additive 增 gate 字段）。
- ✅ 三条闸门规则逐字一致：强判决缺 gate 拒 / against·alternatives 空拒 / Affirmed+Speculative 拒；`requires_self_deception_gate` 仅 Affirmed/Refuted/Fundamental，Provisional 不卡。
- ✅ `render_verdict` 在 append 前 `validate_self_deception_gate`。
- ✅ `branch.rs` 改动全在 `#[cfg(test)]`，非测试逻辑未动。
- ✅ `latest_verdict_for` 在 gate payload 下回归通过。

结论：合并就绪。

## 目标

把 §15 的防自欺协议从「建议」升级为**判决落库前的强制闸门**：强判决（Affirmed / Refuted / Fundamental）写入前必须带合法 `SelfDeceptionGate`，否则拒绝。仅卡高风险声明，**不拖慢** Provisional。纯 `agentflow-core` 库 + 测试，无 CLI。

## 硬约束（与既往一致）

1. 不新增任何 crate 依赖；JSON 手写复用同款私有助手。
2. 事件溯源；不新增 event_type（沿用 `argument.verdict_rendered`，payload 增加 gate 字段，**additive**）；不新增表。
3. `StorageError` 校验风格仿 `research.rs`。
4. `#[cfg(test)] mod tests` 覆盖 ≥80%：每条闸门规则的通过 + 拒绝。
5. 质量门全绿：`clippy -D warnings` + `cargo test`。**基线 core 135 / cli 22 / schemas 3，不得破坏。**
6. `unsafe_code = forbid`。

## 交付物（扩展 `crates/agentflow-core/src/argument.rs`）

```rust
pub enum ClaimBasis { Observed, StatisticallyInferred, Speculative }  // as_str/parse

pub struct SelfDeceptionGate {
    pub supports: String,
    pub against: String,
    pub alternatives: String,
    pub data_quality_risks: String,
    pub assumptions: String,
    pub falsifier: String,
    pub claim_basis: ClaimBasis,
    pub not_yet_claimable: String,
}
```

**修改 `render_verdict` 签名**，增加 `gate: Option<SelfDeceptionGate>` 参数：

```rust
pub fn render_verdict(
    &self,
    hypothesis_id: &str,
    engine: &dyn ArgumentEngine,
    gate: Option<SelfDeceptionGate>,
) -> Result<VerdictReport, StorageError>;
```

闸门强制规则（在 `engine.render` 得到 verdict 之后、`append_event` 之前校验）：

- **需闸门的强判决**：`Verdict::Affirmed`、`Verdict::Refuted`、`Verdict::Inconclusive(InconclusiveKind::Fundamental{..})`。
- **不需闸门**：`Verdict::Inconclusive(InconclusiveKind::Provisional{..})`（gate 可为 None；若提供也接受并存储）。
- 规则：
  1. 强判决而 `gate` 为 `None` → `StorageError::InvalidInput("strong verdict requires self-deception gate")`。
  2. 强判决的 `gate.against` 或 `gate.alternatives` 为空 → `InvalidInput`（防自欺核心：必须列出反证与替代解释）。
  3. `verdict == Affirmed` 且 `gate.claim_basis == Speculative` → `InvalidInput("speculative basis cannot affirm")`。
- 校验通过后，`argument.verdict_rendered` 的 payload **追加** gate 字段（claim_basis + 各文本，存在时）。`latest_verdict_for`（H2）仍只读 hypothesis_id/verdict/confidence，**不受影响**——确认其仍正常工作。

### 同步更新调用点（保持全绿）

`render_verdict` 现有 4 个调用点需加第三参数：
- `crates/agentflow-core/src/argument.rs` 测试 3 处（1053 / 1108 / 1117 附近）。
- `crates/agentflow-core/src/branch.rs` 测试 1 处（422 附近）。
- 规则：Provisional 场景传 `None`；若某测试断言/依赖强判决，则构造合法 gate 传入。**只改这些调用点（多为测试）；不得改动 branch.rs 的非测试逻辑。**

## 验收标准（Claude 审核逐条核对）

- [ ] clippy `-D warnings` 无警告；`cargo test` core 较 135 净增，全绿（含 branch.rs 测试仍绿）。
- [ ] 无新依赖、无新表、无新 event_type。
- [ ] 三条闸门规则各有通过 + 拒绝测试。
- [ ] Provisional 传 None 可正常落库（不被闸门拦）。
- [ ] `latest_verdict_for` 在 payload 增加 gate 字段后仍正确解析（回归测试或现有测试覆盖）。
- [ ] grep 确认未引入新依赖、未改 branch.rs 非测试代码。

## 不在本里程碑（明确排除）

CLI、与主循环集成、把闸门应答接入报告渲染（→ 报告/CLI 里程碑）、Dialectical critique 的额外结构（§15 提及，本步只做八问中的核心子集 + claim_basis）。
