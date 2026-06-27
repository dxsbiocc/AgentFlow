# 验证记录:flow plan(分波执行计划预览,不执行)

Date: 2026-06-26
Status: PASS — `flow plan <flow-id>` 显示调度器**会怎么跑**这条流(逐波、每波哪些步并行),但**不执行任何步骤**。执行层 UX:跑之前先看清并行宽度与拓扑层级。

## 改动

- 核心 `ProjectStore::plan_flow(flow_id) -> Vec<Vec<String>>`:纯拓扑分波。每个内层 vec 是一波(波内步骤互不依赖、可并行);波按序排;已完成步排除;依赖永不满足的步省略。波内顺序 = 调度器(`RuleBasedStepScheduler`)的发射顺序。
- CLI `flow plan <flow-id> [--json]`(复用 `FlowInspectArgs`)。
- usage 增一行。

## 关键修复(merge 前自己抓到的)

第一版直接复用 `ready_steps` + 一个本地 `completed` 集——**死循环**:`ready_steps` 靠步骤的 **DB status** 排除已完成步,而 plan 不真跑、status 不变,于是每轮返回同一批 draft 步,`waves` 无限增长(live demo 卡死、进程被 kill)。改成纯模拟:本地 `done` 集 + 每波从 `remaining` 移除已规划步,保证收敛。新增**核心单测**专门防回归。

## 证据

- 单测 `plan_flow_returns_topological_waves_without_running`:diamond 流(root → {a,b} → join)→ `[[root],[a,b],[join]]`(中间波 a||b),且**终止**。
- **Live**(CLI):线性流 one→two →
  - human:`Waves: 2 (max parallel width 1) / wave 1: one / wave 2: two`
  - json:`{"schema_version":"agentflow.flow_plan.v0","flow_id":"lin","waves":[["one"],["two"]]}`
- `argument.rs` 空 diff;core **363**(+1)+ cli + clippy + acceptance 绿;无新依赖(CLI 端手写 JSON,未引 serde_json)。

## 边界

- 只读、不改状态;`run`/`agent run` 才真跑。
- 计划反映“下次调用会跑什么”:已完成步排除,failed 步会被当作可重试纳入(与运行时一致)。
