# 验证记录:一等公民 module —— 内联展开引擎(slice 1)

Date: 2026-06-28
Status: PASS — 新增 `ModuleSpec`(可复用、typed 的子流程)+ **内联展开**原语:把 module 的内部步骤拷进 flow,id 与内部 artifact 名按实例加前缀,外部输入端口改写到调用方绑定,外部输出端口暴露回调用方。纯 flow-draft 变换,现有调度器/运行时**零改动**即可跑展开后的扁平 DAG。这是"一等公民 module(组合复用)"的**第一片(基础原语)**。

## 动机

任务管理层缺"可复用的命名子流程":flow 是 agent 临时搭的扁平 DAG,无法把"RNA-seq QC→定量"注册成一个可调用单元。内联展开避开运行时递归——module 展开成普通 step 后,复用既有 `run_flow_with`/调度器。直接延续用户最初"注册为独立 module 分析任务、agent 生成工具链路"的意图。

## 设计(本片)

- `ModuleSpec { schema_version(agentflow.module.v0), namespace, name, version, description, inputs: Port{type}, outputs: Output{type, from}, steps: Vec<FlowStepDraft>, source_text }`。复用 `FlowStepDraft`,内部 step 就是普通 flow step。
- **接线约定(无 sigil,与 flow 一致)**:module 内的 artifact 命名空间 = 外部输入端口名 ∪ 内部 step 输出 artifact 名。内部 step 的 input 值若等于某外部输入端口名 → 该端口;否则必须是内部 step 产出的 artifact。所有外部数据只经声明端口进出。
- `expand(instance_id, bindings) -> ModuleExpansion{ steps, outputs }`:
  - step id、内部 artifact 名 → `"{instance}__{local}"`(前缀,**两份同 module 实例不撞名**);
  - `needs` → 前缀化;
  - input 值是外部端口 → 替换为 `bindings[port]`;是内部 artifact → 前缀化;
  - 外部 output 端口 → `outputs[port] = "{instance}__{from}"`(暴露命名空间化的内部 artifact 供父 flow 继续接线)。
- 校验:`from_simple_yaml` 解析即 `validate()`——step id 唯一、端口/输出/artifact 名都是合法 ident、每个 output.from 有内部 producer、内部 input 解析到端口或内部 artifact(否则悬空报错);**经独立审查加固**:① 禁止输入端口名与内部 artifact 名同名(无 sigil 的接线约定下会歧义);② 同一 artifact 名不可被两个 step 产出;③ 内部 input 引用某 artifact 的 step 必须在 `needs` 里声明其 producer(否则调度可能乱序);④ `needs` 环检测(DFS)。`expand` 是 valid-by-construction(不再重复 `validate`),只校验本次绑定齐全且无未知端口。

## 证据

- 单测 12 个(`module_registry.rs`):解析+校验;展开(前缀 id、外部输入改写、内部接线保留、output 映射);两实例不撞名;零输入 module 可展开;缺/未知绑定被拒;output.from 无 producer 被拒;悬空内部 input 被拒;**审查加固项**:端口/artifact 同名被拒、重复 producer 被拒、缺 needs 被拒、needs 环被拒、未知顶层字段被拒。
- 示例 `examples/modules/qc_then_quantify.module.yaml`。
- core **375**(+6)+ clippy(workspace)+ acceptance 绿;`argument.rs` 空 diff;无新依赖(schemas 仅加一个 const)。

## 边界 / 后续片

本片是**纯库原语**,尚未接入:① module 存储 + `module register/list` CLI;② flow YAML 里 `module: <ref>` 作为一个 step,在 approve 时展开(父 flow 的外部端口接线 + 跨实例 needs);③ agent 自动选用 module 组合。逐片小 PR 推进。
