# 验证记录:证据广度 — 撤稿感知(retracted → Unsupported)

Date: 2026-06-25
Status: PASS — foraged 来源可标记为**已撤稿/撤回**,撤稿来源无论访问级别一律 `Unsupported`,agent 不再信任已撤稿文献。延续 #99 的"诚实分级"主题。

## 动机

#99 让预印本能被诚实使用(全文封顶 `Hypothesis`)。证据广度的另一面是**剔除坏来源**:一篇已撤稿论文即便是开放获取全文,也不应支撑任何结论。这是"用更广文献"时必须的诚实护栏。

## 改动

- `ForageObservation` + `ForageObservationPayload` 加 `retracted: bool`;payload 字段标 `#[serde(default)]`,**旧事件(无该键)解析为 `false`**,向后兼容。
- `record_forage_observation` 增 `retracted` 参数并写入 payload;`forage_observation_from_event` 读回。
- 新增 `grade_for_forage_observation(&observation)`:`retracted` 为真 → `Unsupported`,否则走 `grade_for_forage_source(access, source_id)`(#99 的预印本封顶逻辑)。`link_forage_evidence` 改走它。
- CLI:`forage observe --retracted`(布尔旗标,默认 false);`usage` + `forage show`/`format_forage_observation` 显示 `Retracted: true`;auto-forage 的 `ingest_forage_hits` 默认 false。

诚实联动:`Unsupported` 不能支撑判决 → 撤稿来源对 verdict 零贡献,与 grade-cap / 诚实不变量一致。

## 证据

- 单测:撤稿 + `OpenAccessFullText` → `Unsupported`;非撤稿不变;无 `retracted` 键的旧 payload 解析为 `false`(向后兼容)。
- **Live**(agent CLI):同一假设下 forage 一篇 `--retracted` 的 pubmed 全文 + 一篇正常 pubmed 全文并 link —— `evidence list`:
  - `source=PMID:RETRACTED  grade=unsupported`
  - `source=PMID:GOOD       grade=literature_supported`
- `argument.rs` 空 diff;core **360**(+2)+ cli + acceptance 绿;clippy 干净;无新依赖。

## 协作说明

本次实现由 **codex(前台)** 完成代码改动并跑 build + forage 测试;codex 在 commit 前触达用量上限,改动留在工作树。我**逐文件审查了完整 diff**(forage.rs 结构 + serde 默认、CLI 旗标、usage、auto-forage 路径、report.rs 测试),跑了完整门禁(core/cli/clippy/acceptance)+ live 验证后才提交。

## 边界(诚实)

- 撤稿状态是**人工/agent 标记**的(`--retracted`),未自动查撤稿数据库(Retraction Watch 等)——后续可加。
- "预印本→正式发表的升级追踪"未做(本次只做撤稿这一半)。
