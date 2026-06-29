# 验证记录:agent 自治组合已注册 module(slice 4b-2)

Date: 2026-06-29
Status: PASS — 自治 agent 反向链时,若没有 tool 能产出所缺输入类型,会改用一个**已注册的 module**(全输入可用的 High-fit 原子 producer)作为 producer,内联展开进 flow 并接好线。`argument.rs` 字节不变。

## 动机

4b-1 给了发现原语 `match_modules`。4b-2 把它接进 agent 的 `chain_producer_steps_rec`:让"agent 自动选用 module 组合工具链路"真正发生——module 现在能像 tool 一样被自治选中并展开。

## 接线契约(关键)

两套世界对接:agent 用 `ProposedStep`(`"producer.port"` 接线,`infer_step_needs` 既保留显式 needs 又从 `"a.b"` 输入推导 need),module 展开成 `FlowStepDraft`(artifact 名接线 + 显式 prefixed needs)。做法:
- `module_producer_steps(expansion, output_port)`:把展开的 `FlowStepDraft` 转成 `ProposedStep`(保留 prefixed needs),并定位承载该外部输出端口的(内部 step id, 输出端口名)。
- consumer 的缺失输入 → `"{producing_step}.{output_port}"`(走 stepid.port,自动推导 consumer→module 的 need,与 tool 路径一致)。
- `rewrite_module_internal_inputs_for_graph_patch`:把 module 内部 step 间的 artifact 名输入改写成 `"{sibling}.{port}"`,使内部 needs 也经 graph-patch 路径的 `infer_step_needs` 正确推导(外部绑定的 `artifact_*` id 不会被改写——module 输出 artifact 名都是 `"{instance}__..."`,不冲突)。

## 行为 / 安全

- **tool 优先**:仅当 tool 候选都无法 ground 该输入(`grounded_input==false`)才试 module。
- **仅 High-fit**:module 全部输入端口在 `available` 里(原子 producer,本片不递归链入 module 输入)。
- **全有或全无**:`visited` 快照/恢复覆盖每条失败路径(输入不可绑定 / expand 失败 / 定位输出失败);consumer 的 `*value` 仅在所有可失败步骤成功后才改写,无半改写路径。
- instance id = `sanitize("{consumer_step}__{input_name}")`,每个(consumer,input)唯一,两个实例前缀不撞 id。

## 证据

- 单测 `module_producer_steps_maps_expanded_steps_and_external_output_port`:2 步 module 展开 → 2 个 ProposedStep(prefixed id/needs)+ 正确的 (producing_step, port)。
- **集成测试 `module_producer_chain.rs`(端到端 live)**:注册 `bio/qc`、`bio/quantify` 两个真实 tool + 注册 module `bio/qc_then_quantify`(RawCounts→ExpressionTable),不注册任何单独产出 ExpressionTable 的 tool;import RawCounts+SurvivalTable + 建假设 → `agent run --apply --auto-run` → 输出显示 module 内部 step(实例前缀 `__qc`/`__quant`)跑了、consumer 也跑了 → 证明 agent 选中并展开了已注册 module。
- core **396** + cli(含集成测试)+ clippy(workspace)+ acceptance 绿;`argument.rs` 空 diff;无新依赖。

## 协作 & 审查

由 **codex(后台 spawn 但本次确实落地了改动)** 实现;它额外加了 `rewrite_module_internal_inputs_for_graph_patch`(让内部 needs 经 stepid.port 推导,稳健)。我逐行核了 `chain_producer_steps_rec` 集成 + `visited` 纪律 + 全门禁,并跑了独立 `code-reviewer`(本片是 agent loop 手术,重审)。

## 后续(4b 余下)

- **4b-3**:module 作为顶层 answer 候选(`enrich_branch_proposal`)+ 等价分支刹车感知 module 实例。
- **4b-4**:递归链入 module 未满足的输入;嵌套 module。
