# 验证记录:`--max-chain-depth`(可配置自治链深度)

Date: 2026-06-27
Status: PASS — `agent run --max-chain-depth N` 控制自治反向链的最大层数(默认 4)。`0` 关闭链式合成(工具只在输入直接可用时才跑,严格/可预测模式);更深值支持更长的生产者级联。默认行为不变。

## 动机

多层反向链(#95)的深度此前硬编码为 `MAX_CHAIN_DEPTH = 4`。把它做成 `agent run` 的可调旋钮,让用户按需收紧或放宽自治:
- `--max-chain-depth 0`:不自动链生产者——匹配到的工具若缺输入就不跑(无"魔法"、可预测)。
- `--max-chain-depth 1..`:允许的级联层数。

## 改动

- 核心:`ApplyConfig.max_chain_depth: usize`(默认 `DEFAULT_MAX_CHAIN_DEPTH = 4`,pub const)。深度从 `enrich_branch_proposal` → `chain_producer_steps` → `chain_producer_steps_rec`(`depth >= max_chain_depth` 用参数替代原 const)。等价分支刹车的链计算仍用默认深度(只需链消费的类型,与 apply 时设置无关)。
- CLI:`agent run --max-chain-depth <n>`(`parse_usize_value` 允许 0);usage 增一行。
- `argument.rs` 不动。

## 证据

- CLI 测试 `agent_max_chain_depth_bounds_producer_chaining`:两级梯子(RawCounts→MidCounts→ExpressionTable)在 `--max-chain-depth 1` 下**不成链**(深层 producer `upper` 不跑、consumer 因缺输入不跑);默认深度下 3 步成链(既有测试覆盖)。
- **Live**(单生产者 RawCounts→ExpressionTable):`--max-chain-depth 0` → producer 步骤跑了 **0** 次(关链);默认深度 → **1** 次(开链)。
- core 363 + cli(含新测试)+ clippy(workspace,加了 `#[allow(too_many_arguments)]` 给 enrich)+ 两个 acceptance 脚本绿;无新依赖。

## 边界

- 深度只约束自治链;直接可用输入的工具不受影响。
- `0` = 严格模式(无链);默认 4 与之前一致。
