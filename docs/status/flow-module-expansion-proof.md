# 验证记录:一等公民 module —— flow 引用 module 并展开(slice 3)

Date: 2026-06-29
Status: PASS — flow YAML 现可把一个 `module: <ref>` 当作 step,在解析时**内联展开**成普通 tool steps,并正确改写**跨实例 needs**与**输出引用**。展开后是扁平 tool-only flow,既有 validate/approve/调度器/运行时**零改动**直接跑。这是"一等公民 module"的第 3 片(把 module 真正组合进 flow)。

## 用法

```yaml
steps:
  - id: prep
    module: bio/qc_then_quantify      # 引用 module(替代 tool:)
    needs: [importer]                 # 模块的上游依赖
    inputs:
      counts: importer_out            # 绑定 module 的输入端口
  - id: analyze
    tool: bio/analyze
    needs: [prep]                     # 依赖整个模块实例
    inputs:
      expression: prep.expression     # 引用模块实例的输出端口
```

## 设计

- `FlowDraft::from_simple_yaml_with_modules(source, &modules)`(老的 `from_simple_yaml` = 空 module 表,遇到 `module:` step 直接报错"未提供")。`RawFlowStepDraft` 加 `module` 字段。
- `resolve_flow_steps`(两趟):
  1. 每个 `module:` step → `ModuleSpec::expand(step.id, step.inputs)`(实例 id = step.id,inputs = 端口绑定),内联 steps 拼入;module step 自己的 `needs` 下放给实例的**源步骤**(展开后 needs 为空者);记录实例的输出端口→(暴露 artifact, 产出步骤)与 sink 步骤。
  2. 对**所有** step 改写:`needs` 里指向某实例 id 的 → 替换为该实例的 sink 步骤;input 值形如 `instance.port` → 替换为暴露 artifact 并把产出步骤加进 `needs`。`dedupe_in_place` 保序去重。
- 单一/组合调用:单个 = 一个 module step;组合 = 多个 module step + tool step 自由混排,按 `instance.port` / `needs:[instance]` 接线。

## 排序正确性(审查确认 1a/1b/1c 全对)

- (a) 下游消费 `instance.port` → 必然获得到产出步骤的 `needs` 边(artifact 与 producer 成对存储,不可能只解析到 artifact 而漏掉 needs);
- (b) `needs: [instance]` → 等到模块暴露输出的产出步骤(无暴露输出则等所有内部步骤);
- (c) 模块的源步骤继承 module step 的上游 `needs`。

## 证据

- 单测 7 个(`flow_registry.rs`):展开 + 下游接线;上游 needs 下放到源步骤;两实例不撞名;未知 module 被拒;同时声明 tool+module 被拒;无 module 表时 `module:` step 被拒;**点号实例 id 被拒**。
- core **389**(+7)+ clippy(workspace)+ acceptance 绿;`argument.rs` 空 diff;无新依赖。

## 协作 & 审查

跨实例 needs 改写是本特性最易出错处,故**我直接实现**(未交 codex),再用 `code-reviewer` 独立审查。两个 HIGH **已修并加测试**:① 点号 module 实例 id 会让 `instance.port` 解析错位 → 解析期拒绝;② artifact→producer 查找与端口校验结构上解耦(理论可漏 needs 边)→ 改为端口→(artifact, producer) **成对存储**,不可能解耦。

## 后续片

- slice 3b:CLI `flow create --module <file>...`(把 module YAML 喂给 flow 创建);
- slice 4:agent 自动选用 module 组合。
- 暂不支持嵌套(module 内部 step 仍是 tool)。
