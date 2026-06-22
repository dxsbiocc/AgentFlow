# RFC: Agent 智能调度(v0.2.0 P2)

Status: Draft (P2 设计基线)
Scope: `crates/agentflow-core/src/runtime/`(`ready_steps`/`run_flow`)、新 `runtime` 调度 seam、`crates/agentflow-cli`(`agent run`/`run` 报告)
North star: [docs/CAPABILITIES.md](../CAPABILITIES.md) 诚实性不变量不变;`argument.rs` 判决出口 0-LLM/0-网络不变;**调度只改执行顺序,绝不改结果**。
前置:隔离执行引擎 P1(#69–#73)+ 组合 live 证明(#74)。

## 1. 愿景(用户重申 #1)

> agent 决定/调度任务执行——调度是一等智能层,不止静态 DAG 跑。

组合基底已 live 证明(#74:生产者→消费者经声明 I/O + staging 端到端跑通)。但那条流是**人手写 YAML + 人填 ID**。P2 的目标是让 **agent 来排程**:多个就绪步竞争时按价值/优先级智能决定先跑谁,而非声明顺序。

## 2. 现状(已具备 / 缺口)

- **`ready_steps`(runtime/mod.rs:1646)**:计算就绪步(依赖已完成),但**按声明顺序**返回——无优先级、无智能。
- **`run_flow`**:消费 `ready_steps`,按该顺序跑。
- **`BranchSelector` / `RuleBasedSelector`(branch.rs:157-174)**:跨假设的**打分排序 seam**已存在(`BranchCandidate.score` desc + 稳定 tie-break)。**这是要复用的模式**——确定性 rule-based 排序 + 未来可挂 LLM/价值 scorer seam。
- **缺口**:流内就绪步无原则排序;无 agent 驱动优先级;无价值调度;并行/取消未做;跨流/多假设调度未做。

## 3. 设计原则

1. **镜像 BranchSelector 模式**:调度是确定性 rule-based seam(core 内,0-LLM),价值/LLM scorer 是**未来 seam**(Noop/rule 默认)。与系统"确定性核心 + 可选 LLM seam"一致。
2. **调度只排序,不改结果(关键不变量)**:就绪步之间相互独立(依赖已满足);调度只决定**先后/并发**,**绝不改变任一步的输出或缓存键**。回归测试必须断言:不同调度顺序 → 相同最终工件/结果。
3. **确定性**:rule-based 调度对同一图给出确定性顺序(可复现、可审计)。
4. **声明式优先级**:flow step 可选 `priority`(整数,高者先),与既有 `infer:` hint 同款声明式风格。

## 4. 目标模型

### 4.1 `StepScheduler` seam

```
trait StepScheduler {
    /// 对就绪步给出执行顺序(确定性)。输入含图结构以便算结构信号。
    fn order(&self, ready: Vec<StoredFlowStep>, graph: &FlowGraphView) -> Vec<StoredFlowStep>;
}
struct RuleBasedStepScheduler;   // 默认
```

`RuleBasedStepScheduler.order` 排序键(确定性):
1. 声明 `priority` 降序(缺省 0)。
2. **下游解锁数**降序(该步完成后能解锁多少后继步——优先跑"解锁更多工作"的步,近似关键路径)。
3. 声明顺序(稳定 tie-break)。

`run_flow` 用调度器对 `ready_steps` 结果排序后再执行。

### 4.2 未来 seam(P2 之后,本 RFC 不实现)

- **价值调度 scorer**:LLM/启发式按"该步对推进假设的期望证据价值"给优先级(Noop 默认 → 不改确定性);仅作为 priority 的上游建议,最终顺序仍由确定性 scheduler 落定 + 可审计。
- **并行调度 + 取消**:就绪集并发执行 + 取消控制(需隔离 P1 作前提——已具备)。
- **跨流 / 多假设调度**:把 BranchSelector(跨假设)与 StepScheduler(流内)统一为一个"下一步该跑什么"的智能层。
- **自治依赖接线**:applied step 自动推断 `needs` 边(README "not wired" 缺口),让 agent 自己长出可调度的多步图——与组合基底(#74)直接衔接。

## 5. P2.1 第一切片(最窄可跑)

**范围:`StepScheduler` seam + `RuleBasedStepScheduler` + 可选 step `priority` + `run_flow` 接线。** 不含并行/取消/LLM scorer/自治接线。

交付:
1. `runtime` 内新增 `StepScheduler` trait + `RuleBasedStepScheduler`(确定性:priority → 下游解锁数 → 声明顺序)。
2. flow step 可选 `priority: <int>`(schema 向后兼容,缺省 0;解析 + 进 StoredFlowStep)。
3. `run_flow` 用调度器排序就绪步;`run` 报告/JSON 可显示就绪步的调度顺序(可选)。
4. 文档:CAPABILITIES 增"调度"节;README 能力表。

**不变量(硬约束)**:
- `git diff crates/agentflow-core/src/argument.rs` 为空;调度器在 core 内但 **0-LLM**。
- **调度只排序不改结果**:新增回归测试断言——同一图、两种 priority 配置 → **相同最终工件 + 相同 step 缓存键**(顺序变,结果不变)。
- `priority` 是**可选**字段,旧 flow YAML 向后兼容。
- 无新依赖;现有 `examples/` + acceptance 全绿(默认 priority=0 → 行为退化为"下游解锁数 + 声明顺序",对单链流与现状一致)。

## 6. 实施切分(给后续苦力活)

- **P2.1a**:`StepScheduler` trait + `RuleBasedStepScheduler` + `run_flow` 接线(就绪步排序),先不加 priority 字段——纯结构信号(下游解锁数 + 声明顺序),行为对现有单链流等价。← 先做,零 schema 改动、零行为变化风险。
- **P2.1b**:加可选 `priority` step 字段 + 解析 + 排序键首位;回归测试(顺序变结果不变)。
- 之后据 live 表现再开 P2.2(并行/取消)或价值 scorer seam。

每片可独立 review/合并;P2.1a 先行(P2.1b 依赖它)。

## 7. 与初心 / 既有边界的关系

- 沿用 tool-evolution / isolated-execution RFC 的治理风格:小步、可 review、每步守不变量。
- **吸取上一轮教训**(infra 不得快于验证):每片都要有"调度只排序不改结果"的 live/离线证据,不堆没验证的调度复杂度。
- 调度建立在已 live 证明的组合基底(#74)之上;价值调度 / 并行 / 自治接线是再往后的 seam,均 Noop 默认以守确定性。
