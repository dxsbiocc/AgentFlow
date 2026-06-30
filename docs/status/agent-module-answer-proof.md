# 验证记录:agent 用 module 直接回答假设(slice 4b-3b)

Date: 2026-06-30
Status: PASS — 当没有 tool 能回答某假设时,自治 agent 会改用一个**已注册、能产出 observation 的 module**(其内部某 step 是带 observer 输出的 tool)作为答案:展开 module,把那个 observed step 当 `drafted_step`(答案),其余当 `prerequisite_steps`。`argument.rs` 字节不变。

## 动机

4b-2 让 module 能当 producer;4b-3a 给了"哪些 module 能回答"的发现原语。4b-3b 把它接进 agent 顶层匹配 `enrich_branch_proposal`,让 module 能直接**回答**假设(而不只是当中间 producer)。

## 设计(低爆炸半径:早返回)

在 `enrich_branch_proposal` 算出 top tool 候选后:
- **无 tool 回答**(`!top.answer_priority`)→ 调 `module_answer_proposal`,有则早返回。
- (额外,codex 自加且合理)**tool 回答但 drafted step 仍有未解析的必需 input/param**(`has_unresolved_required_inputs`/`has_unresolved_required_params`)→ 也回退到 module answer(仅当存在;否则照旧走 tool)。一个接不通的 tool 答案不如一个能落地的 module 答案。

`module_answer_proposal`:取第一个 High-fit 的 `answer_capable_modules` 候选,按类型把 module 输入端口绑定到可用 artifact,`expand`,转 `ProposedStep`,`rewrite_module_internal_inputs_for_graph_patch`(内部 artifact 名输入→`stepid.port`),把 `"{instance}__{answer_step}"` 这个 observed step 取出当 `drafted_step`(并校验它确有 `observer_port` 输出),其余当 `prerequisite_steps`。

**关键不变量**:返回的 `EnrichedProposal` 与 tool 答案**形状一致**——`matched_tool` = 那个 observed step 的**真实 tool ref**(让下游 observation 记录照常工作),`matched_fit` = "high"。因此 `auto_synth_gap` 为 false(synth 不触发)、刹车/apply 路径**不变**。instance_id = `sanitize("{hypothesis_id}__{module_ref}")`,每(假设,module)唯一。

## 证据

- core **398** 全绿(额外回退未回归既有 tool 答案路径——仅在 module 存在时才回退)。
- **端到端 live 集成测试 `module_answer_chain.rs`**:注册 module 内部 tool(`bio/prep` 产 ExpressionTable、`bio/report` 带 observer)+ 注册 module `bio/assoc_report`,**不注册任何能回答该假设的独立 tool**;import 输入 + 建假设 → `agent run` → 那个带 observer 的 module 实例 step(实例前缀)跑了并产出 observation,假设被回答 → 证明 agent 经 module 路径回答。
- clippy(workspace)+ acceptance 绿;`argument.rs` 空 diff;无新依赖。

## 边界(诚实)

- **module 答案 step 的 params 是 module spec 里固定的**——本片不做"按假设推导 module 内部 param"(如按假设填 gene)。要按假设参数化的 module 答案是后续工作。
- 仅 High-fit module 答案(全输入直接可用);链入 module 未满足输入 = 4b-4。
- 刹车 `has_equivalent_tool_branches` 未改(它只排 match_tools 候选;module 答案仅在无 tool 回答时出现)。

## 协作 & 审查

codex 实现(异步 spawn 落地;额外自加了"tool 接不通则回退 module"的合理增强 + `module_answer_proposal`/`module_expansion_steps`/`has_unresolved_*` helpers)。我逐行核了早返回 + helper + 全门禁,并跑独立 `code-reviewer`(agent loop 手术,重审)。
