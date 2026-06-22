# 简报：P2.1a 就绪步调度 seam(结构信号,行为等价)

Status: Assigned to Codex（worktree /tmp/af-p21a，branch feat/p21a-step-scheduler，从 main 起）
RFC: docs/design/agent-scheduling-design.md §4.1 / §5 / §6(P2.1a)。

## 现状(crates/agentflow-core/src/runtime/mod.rs)

- `run_flow`(:625):循环里 `let ready = ready_steps(&flow.steps, &flow.edges, &completed);`(:639),按返回顺序执行就绪步。
- `ready_steps`(:1646):返回就绪步,**按 `steps` 声明顺序**(无优先级/无智能)。

## 目标(本切片,纯结构、行为等价)

引入一个 `StepScheduler` seam(镜像 `branch.rs` 的 `BranchSelector`/`RuleBasedSelector` 模式),对就绪步给出**确定性**执行顺序;`run_flow` 用它排序就绪步。**本切片不加 priority 字段、不改 schema**——排序键纯结构:

1. **下游解锁数降序**:该步完成后能解锁多少后继步(用 edges 算 from_step_id==该步 的后继计数),优先跑"解锁更多工作"的步(近似关键路径)。
2. **声明顺序**(稳定 tie-break)。

→ 对现有**单链流**(每轮只一个就绪步)顺序与现状完全一致 → **行为等价**。

## 实现要求

1. 在 `runtime`(`mod.rs` 内或新 `runtime/schedule.rs` 子模块,`mod schedule;`)定义:
   ```
   pub(crate) trait StepScheduler {
       fn order(&self, ready: Vec<StoredFlowStep>, edges: &[StoredFlowEdge]) -> Vec<StoredFlowStep>;
   }
   pub(crate) struct RuleBasedStepScheduler;
   ```
   `RuleBasedStepScheduler.order`:对 ready 按(下游解锁数 desc,声明顺序 asc)稳定排序。下游解锁数 = `edges.iter().filter(|e| e.from_step_id == step.id).count()`(或更精确:解锁后变就绪的后继数,首版用直接后继计数即可,注释说明)。
2. `run_flow`:`let ready = RuleBasedStepScheduler.order(ready_steps(...), &flow.edges);` 再执行。保持其余逻辑不变。
3. 不改 `ready_steps` 的语义(仍算就绪集);调度器只重排。

## 不变量(硬约束)

- `git diff crates/agentflow-core/src/argument.rs` 为空;调度器 **0-LLM/0-网络**。
- **行为等价**:所有现有 `run_flow_*` 测试**断言不改**即通过(单链流顺序不变)。
- **调度只排序不改结果**:新增一个测试——构造一个有**两个并列就绪步**(无相互依赖,如 diamond 头部两分支)的流,断言调度顺序确定(按下游解锁数),且**两步的输出工件与现状一致**(顺序不改变结果)。
- 仅改 `crates/agentflow-core/src/runtime/`(mod.rs + 可选 schedule.rs);不碰 storage schema、不碰 CLI、不碰 argument.rs。
- 无新依赖;`unsafe_code=forbid` 不破。

## 测试(离线,低负荷)

- 现有 `run_flow_*` 测试原样通过(等价证明)。
- 新增:`RuleBasedStepScheduler.order` 单测——多个就绪步按(下游解锁数 desc,声明序)确定排序;一个就绪步时返回原样。
- 新增 run_flow 级:两并列就绪步的流跑通,产出工件与单一顺序一致(顺序无关结果)。
- 跑:`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test -p agentflow-core`、`bash scripts/acceptance-v1.sh`。**不要** `cargo test --workspace`。

不要 commit。报告:scheduler 位置、run_flow 接线、确认现有 run_flow 测试未改即过 + argument.rs 未动 + acceptance 绿。
