# 验证记录:module 持久化存储 + register/list(slice 4a)

Date: 2026-06-29
Status: PASS — module 现可注册进项目 DB 并列出/取回,镜像工具注册表。这是 slice 4(agent 自动组合 module)的**前置基础**:agent 要"自动选用 module"先得能**发现**已注册的 module。

## 改动

- **迁移 v2**:新增 `modules` 表(`schema.rs` 的 `MODULES_TABLE_SQL` 常量 + `migrations.rs` 的 `Migration{version:2,...}`)。**不动 `V0_SCHEMA_SQL`**(其 checksum 对已有库做校验,改动会破坏旧项目)——审查重点确认 v1 未变、v2 幂等(`CREATE TABLE IF NOT EXISTS`)、`apply_migrations` 对新/旧库都正确、`validate_applied_migrations` 仍通过。
- **核心**(`module_registry.rs` 的 `impl ProjectStore`):`register_module`(`migrations::checksum(source_text)` 做 spec_hash,SELECT-before-INSERT 定 `replaced_existing`,`INSERT ... ON CONFLICT(id) DO UPDATE`,re-register 保留 `created_at`、更新 `updated_at`)、`list_modules`(按 id 排序)、`get_module`(取 source_text 重解析,缺失返回 `NotFound`)。结构体 `ModuleRegistration`/`ModuleSummary` 镜像 Tool 版,re-export。
- **CLI**:`module register <file> [--path]`、`module list [--json] [--path]`。`--json` 带 `schema_version` 信封(`agentflow.module_list.v0`)。
- `argument.rs` 不动。

## 协作 & 审查(codex + 严格审查发现真 bug)

由 **codex(前台)** 实现,随后 `code-reviewer` 独立审查:迁移正确性(最高优先)**确认无误**,无 CRITICAL/HIGH。3 个 MEDIUM 已修:① list `--json` 缺 `schema_version` 信封 → 加常量 + 信封;② `replaced_existing` 非原子 → 加注释(单连接串行,实践安全);③ 补 CLI 集成测试。**补测时当场抓到一个会上线的真 bug**:`ModuleRegisterArgs` 的位置参数命名 `path` 与 flatten 进来的项目 `--path` **clap 撞名**(debug_assert panic)——改名 `module_yaml`(与 `ToolsRegisterArgs.tool_yaml` 一致)修复。这正印证审查 LOW-1"CLI 接线无测试会漏 bug"。

## 证据

- 核心单测(`module_registry.rs`):register→list 返回正确摘要;get 往返;同 ref bump 版本 → replaced_existing=true 且 list 仍单条新版本;`get_module("nope")` 报 NotFound。
- CLI 集成测试(`module_cli.rs`):`module register` → `module list`(文本 + `--json` 含 schema_version 信封 + ref)往返。
- core **393**(+4)+ cli(含新测试,4 个)+ migrations 5 + clippy(workspace)+ acceptance 绿;`argument.rs` 空 diff;schemas 加一个 const;无新依赖。

## 后续(slice 4 余下)

- **4b**:agent 自治构建 flow 时,把已注册 module 当候选(类型匹配 + 反向链考虑 module 输出端口),自动选用并展开。这是 agent.rs 手术,最敏感、需重审。
- 嵌套 module 仍不支持。
