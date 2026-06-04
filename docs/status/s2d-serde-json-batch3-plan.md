# ②d 实现简报：serde_json 第三批（comparison / report / observer / graph_patch）

Status: Assigned to Codex
Owner: Claude(编排) · Codex(执行)
Depends on: ②a/②b/②c（serde_json 样板，已合并 main）

## 目标
按已验证样板，把 comparison.rs / report.rs / observer.rs / graph_patch.rs 手写 JSON 换 serde_json、删本地重复助手，byte-identical + 旧 payload 兼容。observer(52)/graph_patch(44)/comparison(35) payload 面较大，逐一对照。

## 样板（②a-②c，已验证）
- 事件 payload 构建/解析 → 同名同序 payload 结构体 derive Serialize/Deserialize + serde_json to_string/from_str；旧手写 payload 仍可读。
- 公开记录类型 to_json → serde_json::to_string(self)，字段声明顺序=现有 JSON key 顺序（紧凑 byte-identical）。
- 枚举加 derive+serde rename 使序列化字符串==as_str()。
- 带数据枚举/扁平化的特殊 JSON：用 String 字段镜像现有确切形状，不 naive derive（同 ②c 的 Verdict 处理）。
- 删本模块重复 escape_json/json_string_field/unescape（批外共用的保留并确认）。

## 约束
1) byte-identical / 现有测试不改且通过（首要）；旧手写 payload 仍能 from_str。
2) 领域结构体公开 API、枚举 as_str/parse/逻辑不变。
3) 仅改 comparison.rs/report.rs/observer.rs/graph_patch.rs（+枚举 derive）；不动其它模块；不新增依赖。
4) graph_patch 的 GraphPatchOperation（add_step/add_edge/update_params 的 op JSON）若是扁平/带 tag 形状，按现有确切 JSON 镜像，不改 op 语义。

## 验收
- [ ] clippy -D warnings 干净；cargo test 全绿；acceptance-v1.sh 通过。
- [ ] 四模块现有测试不改且通过；各加 exact-byte to_json/payload 测试 + 旧 payload 兼容测试。
- [ ] 四模块本地 JSON 助手清零。
- [ ] 改动仅这四个 .rs。

## 不在本里程碑
research + 存储层(tool_registry/flow_registry/project_store/artifact_registry) + runtime 的 JSON（②e 收尾）；clap（③）。

## 验收记录（Claude 独立复验 2026-06-04）
- ✅ clippy 干净；cargo test core 223(+7)/cli 61/schemas 3 全绿；acceptance 通过。
- ✅ 改动仅 comparison/report/observer/graph_patch（净删：graph_patch 288/489、observer 297/138、report 18/119、comparison 143/134）；四模块本地 JSON 助手清零。
- ✅ byte-identical：现有测试不改通过 + 新增 exact-byte to_json/payload 测试 + 旧 payload 兼容；graph_patch op JSON(add_step/add_edge/update_params) 用 op:String 扁平 mirror 保持形状。
- ✅ 无新依赖。
结论：合并就绪。剩余 ②e（research + 存储层 + runtime）+ ③ clap。
