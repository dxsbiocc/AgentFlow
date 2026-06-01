# T1 实现简报：工具选择层（Capability 匹配 + ProposedStep 草拟）

Status: Implemented + verified (2026-06-01)
Date: 2026-06-01
Owner(orchestrator): Claude · Executor: Codex
Spec source: [`agentflow-technical-design.md`](../agentflow-technical-design.md) §9 Capability Index（落到实际 ToolSpec 数据）
Depends on: tool registry（已存在）、branch.rs `ProposedStep`（H2，已验收）

## 验收记录（Claude 独立复验 2026-06-01）

- ✅ `clippy -D warnings` 无警告；`cargo test` core **165**（基线 161，+4）/ cli **43**（基线 40，+3）/ schemas 3 全绿。
- ✅ Cargo 零变更；无新表/event_type；**branch.rs 零改动**；现有 `tools register/list/inspect` 与 40 cli 测试未改（lib.rs 唯一删除为 usage 提示文案）。
- ✅ 新建 `tool_select.rs`：`match_tools` 确定性评分（输出类型+10/必填输入+3/关键词 name+4·desc+2/maturity）、`draft_step_for` 复用 `branch::ProposedStep`。
- ✅ **端到端冒烟**：注册 marker 工具 → `tools match --output Markdown --input TSV --keyword survival` → `[high] score=23`；无关查询 `[low] score=1`；`tools draft-step --json` 产出合法 ProposedStep（输入按类型映射、param 占位、输出 id 生成）；缺失工具 NotFound。

结论：合并就绪。工具选择层就位——分支决策可转为具体图补丁，H7b-2 自主 apply 的前置已备（开闸仍待显式授权）。

## 目标

建「工具选择层」：给定一个能力需求（期望输出类型 / 可用输入类型 / 关键词），从 tool registry **确定性打分排序**出候选工具，并为 top 候选**草拟一个具体 `ProposedStep`**。这是 H7b-2 自主 apply 的真前提——让分支决策（Deepen/Spawn）能变成可落地的真图补丁。

## 架构现实（匹配只能基于实际字段）

ToolSpec 没有 capability/tags/domain 字段。匹配只能基于：`description`（文本）、`inputs`/`outputs` 的 `type_name`、`maturity`、`name`。不要臆造新字段。

## 硬约束（与既往一致）

1. 纯 additive：新建 `crates/agentflow-core/src/tool_select.rs` + lib.rs 注册；CLI 在现有 `tools` 子分发**新增** `match`/`draft-step` 子命令（不改现有 tools 子命令 handler/测试）。
2. 不新增依赖；不新增表/event_type（纯读 tool registry + 计算，无需落事件）。
3. 复用 `branch::ProposedStep`；不改 branch.rs 既有逻辑。
4. 质量门全绿：`clippy -D warnings` + `cargo test`。基线 core 161 / cli 40 / schemas 3。
5. `#[cfg(test)] mod tests` 覆盖 ≥80%。

## 交付物

### `crates/agentflow-core/src/tool_select.rs`（新建）

```rust
pub enum Fit { High, Medium, Low }   // as_str

pub struct CapabilityQuery {
    pub desired_output_type: Option<String>,  // 期望输出 type_name，如 "Markdown"/"FusionTable"
    pub available_input_types: Vec<String>,   // 项目现有可用的 artifact 类型
    pub keywords: Vec<String>,                // 与 name/description 匹配
}

pub struct ToolCandidate {
    pub tool_ref: String,    // namespace/name@version 或现有 tool_ref 形式
    pub fit: Fit,
    pub score: i32,
    pub reason: String,      // 确定性说明：命中了哪些维度
}

impl ProjectStore {
    pub fn match_tools(&self, query: &CapabilityQuery) -> Result<Vec<ToolCandidate>, StorageError>;
    /// 为某工具草拟 ProposedStep：必填输入按类型从 available 映射；缺失留占位符。
    pub fn draft_step_for(
        &self,
        tool_ref: &str,
        available: &[(String, String)],  // (type_name, artifact_id)
    ) -> Result<crate::branch::ProposedStep, StorageError>;
}
```

确定性评分（模块常量；`match_tools` 遍历 `list_tools`/`inspect_tool`）：
- 期望输出类型被该工具某 output 的 `type_name` 命中：`+10`
- 每个 available 输入类型能满足该工具一个**必填** input 的 type：`+3`
- 关键词出现在 `name`：每个 `+4`；出现在 `description`：每个 `+2`（大小写不敏感）
- maturity：`verified +3` / `wrapped +1` / `exploratory 0`
- `fit`：输出命中且所有必填 input 可由 available 满足 → `High`；输出命中**或**多数必填 input 满足 → `Medium`；否则 `Low`
- 排序：score 降序，并列按 tool_ref 升序（确定性）。`reason` 拼接命中维度。

`draft_step_for`：
- `id`：`format!("step_{}", 工具 name)`（非法字符规整）。
- `tool`：tool_ref。
- `needs`：空（依赖由调用方设定）。
- `inputs`：对每个必填 input port，在 available 里找 type 匹配的首个 artifact → `(input_name, artifact_id)`；找不到 → `(input_name, "artifact_REPLACE_<input_name>")` 占位。
- `params`：每个必填 param → `(param_name, "REPLACE_<param_name>")` 占位。
- `outputs`：每个 output port → `(output_name, format!("{stepid}_{output_name}"))`。
- 工具不存在 → `inspect_tool` 的 NotFound 透传。

### CLI（现有 `tools` 子分发新增两子命令）
- `tools match [--output <type>] [--input <type>]... [--keyword <kw>]... [--json] [--path <p>]` → 排序候选（人类输出 tool_ref / fit / score / reason；`--json` 数组）。
- `tools draft-step <tool-ref> [--input <type>:<artifact-id>]... [--json] [--path <p>]` → 输出 `ProposedStep`（`--json` 为可被 `propose_branch_patch` 复用的结构）。

## 验收标准（Claude 审核逐条核对）

- [ ] `clippy -D warnings` 无警告；`cargo test` core/cli 较基线净增，全绿；现有 40 cli 测试未改。
- [ ] 无新依赖/表/event_type；不改 branch.rs / 现有 tools 子命令。
- [ ] `match_tools` 评分确定性：输出类型命中、输入满足、关键词、maturity 各有断言；排序稳定。
- [ ] `draft_step_for`：输入按类型映射、缺失占位、输出 id 生成、必填 param 占位、工具不存在 NotFound，各有测试。
- [ ] `tools match` / `tools draft-step` happy-path + `--json` + 错误透传测试。
- [ ] 端到端：注册一个工具 → `tools match --output <type>` 命中它 → `tools draft-step` 产出合法 ProposedStep。

## 不在本里程碑（明确排除）

把工具选择接进主循环让 Deepen/Spawn 自动产出真补丁（→ 后续）、auto-apply 开闸（→ H7b-2，需显式授权）、tool promotion pipeline、新增 capability/tags 字段。
