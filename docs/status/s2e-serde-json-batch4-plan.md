# ②e 实现简报：serde_json 收尾（research + 存储层 + runtime）

Status: Assigned to Codex
Owner: Claude(编排) · Codex(执行)
Depends on: ②a-②d（serde_json 样板，已合并 main）

## 目标
serde_json 最后一批：research.rs / storage/{project_store,artifact_registry,tool_registry,flow_registry}.rs / runtime/mod.rs 手写 JSON 全换 serde_json、删本地重复助手。**迁完此批 core 不应再有任何手写 escape_json/json_string_field（grep 清零）。**

## 关键 byte-identical 面（必须保持）
1) **tool_registry：spec hash 不变**——`marker_survival_scan` = `2f8e22fc89c1caf9`、`tcga_survival_assoc` = `83405af4395dc680`（spec hash 由序列化算出，序列化改了会变 hash → 破坏一切）。这是首要铁证。
2) 所有 CLI `--json` 输出 byte-identical：`tools list/inspect`、`artifacts list/inspect`、`runs list/inspect`、`research list/inspect`、`status`、`cache list` 等。
3) 事件 payload（存储层写大量事件）byte-identical；旧手写 payload 仍能 from_str。

## 样板（②a-②d，已验证）
payload/记录 同名同序结构体 derive + serde_json to_string/from_str；字段声明顺序=现有 JSON key 顺序；带数据/扁平枚举用 String 字段镜像不 naive derive；删本地助手。

## 约束
1) byte-identical / 现有测试不改且通过（首要，spec hash 尤其）；旧 payload 仍可读。
2) 领域结构体公开 API、枚举 as_str/parse 不变。
3) 仅改这 6 个文件（+枚举 derive）；不动其它；不新增依赖。
4) 删本批模块本地重复 JSON 助手；迁完 core 手写 JSON 助手清零。

## 验收
- [ ] clippy 干净；cargo test 全绿；acceptance-v1.sh 通过。
- [ ] **spec hash 不变**（2f8e.../83405af...）；CLI --json 抽样 byte-identical。
- [ ] 现有测试不改通过；各加 exact-byte/旧 payload 兼容测试。
- [ ] grep 确认 core 再无手写 escape_json/json_string_field。
- [ ] 改动仅这 6 个文件。

## 不在本里程碑
clap（③）。

## 验收记录（Claude 独立复验 2026-06-04）
- ✅ clippy 干净；cargo test core 234(+11)/cli 61/schemas 3 全绿；acceptance 通过。
- ✅ 改动仅 6 个文件（净删：tool_registry 277/469、runtime 145/229、artifact_registry 220/122、flow_registry 201/143、research 97/112、project_store 50/38）。
- ✅ **core 手写 JSON 助手彻底清零**（grep 空）——serde_json 迁移全部完成。
- ✅ **spec hash 不变**（Claude 独立复验）：marker 2f8e22fc89c1caf9、tcga 83405af4395dc680；tools inspect --json / artifacts --json 逐字节对照基线一致。
- ✅ 无新依赖。
结论：合并就绪。serde_json 迁移（②a-②e）全部完成；core 再无手写 JSON。剩 ③ clap。
