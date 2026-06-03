# S1 实现简报：受控工具合成（自主写代码 + 验证闸门 + 低信任）

Status: Implemented + verified (2026-06-03)
Date: 2026-06-02
Owner(orchestrator): Claude · Executor: Codex（Rust 管线 + stub 离线测试）· 编排者 live 验证（claude 后端）
Spec source: 用户要求「自主写代码解决问题」—— 自主代码合成的安全闭环第一增量

## 验收记录（Claude 独立复验 2026-06-03）

- ✅ `clippy -D warnings` 无警告；`cargo test` cli **53**（基线 49，+4）/ core **180（未改）** / schemas 3 全绿。
- ✅ 改动仅 `synth_commands.rs`（新）+ lib.rs（dispatch/usage 3 行）；**core 零改动**；Cargo 零变更；无新表/event_type/依赖。
- ✅ 离线 stub 测试：通过→注册 exploratory、不通过→拒绝未注册、synthesizer 缺失→错误、围栏剥离。
- ✅ **真 LLM live 验证（claude -p）**：
  - PASS：`synth --description "count lines" --fixture(5行) --expect 5` → claude **自主写出真实数行脚本**（读 `SYNTH_INPUT`、计数、打印）→ 验证得 5 匹配 → 注册 `synth/linecount@0.1.0 [exploratory]`。
  - REJECT（闸门有牙）：同脚本但 `--expect 99` → 脚本真实输出 5 ≠ 99 → `REJECTED`、`badcount` 未注册。
  - 合成工具可进 runtime（flow 引用 `synth/linecount` 校验通过）。

结论：合并就绪。Agent 现可**自主写代码解决问题**，且**只有过 fixture 验证才被低信任（exploratory）接纳**——写易、验证难，安全机器先立。LLM 接进循环引擎、循环自动触发合成、证据 grade 封顶强制、真沙箱 = 后续 S2+。

## 目标

新增 `agentflow synth` 命令：**让 Agent 在缺工具时自主写代码，但只有过验证才被低信任接纳**。
```
synthesize（LLM 写脚本，后端可配）→ 沙箱验证（fixture 已知答案）→ 通过才注册 exploratory（低信任）；否则拒绝、保留脚本供人查、不注册
```
**写不是难点，验证才是**——S1 的核心交付是「验证闸门 + 低信任注册」这套安全机器，不是 LLM 本身。

## 编排者裁决（约束）

1. **全在 CLI**：新增 `crates/agentflow-cli/src/synth_commands.rs` + lib.rs 加 `synth` dispatch + usage。**core 不改**（复用 `ProjectStore::register_tool` + `ToolSpec::from_simple_yaml`）。
2. **后端可配**：`--synthesizer "<cmd>"`（默认 `claude -p`）。synthesizer 命令按空白拆成 argv，prompt 作为最后一个参数追加；stdout = 候选脚本。**测试用 stub 命令**（指向 fixture 脚本，输出固定的对/错脚本）→ 离线可测两条路径，不依赖真 LLM。
3. **绝不在未验证前信任**：合成脚本必须先在隔离 workdir 用 fixture 跑通且输出匹配 `--expect`，才允许注册；注册一律 `maturity: exploratory`（低信任）。
4. 不新增依赖（子进程用 `std::process::Command`，markdown fence 剥离手写）；不新增 event_type/表。
5. 质量门全绿：`clippy -D warnings` + `cargo test`。基线 core 180 / cli 49 / schemas 3，现有测试不改。

## 交付物：`agentflow synth`

```
agentflow synth --name <n> --description "<做什么>" --fixture <输入文件> --expect <期望输出子串> \
                [--synthesizer "<cmd>"] [--path <project>]
```

流程：
1. **构造 prompt**（固定模板）：要求写一个**仅用 Python3 标准库**、自包含的脚本，从环境变量 `SYNTH_INPUT` 读输入文件路径、把结果写 stdout；任务 = `--description`；**只输出裸代码，无 markdown 围栏**。
2. **调 synthesizer**：`argv(--synthesizer) + [prompt]` 子进程；stdout = 候选脚本；剥离 ```` ```python ```` / ```` ``` ```` 围栏（若有）。
3. **保存**到 `<project>/.agentflow/synth/<name>.py`。
4. **验证**：在临时/隔离 cwd 跑 `/usr/bin/env python3 <script>`，环境 `SYNTH_INPUT=<fixture 绝对路径>`，限时（如 60s），捕获 stdout/stderr。
5. **闸门**：
   - stdout 含 `--expect` → **通过**：生成工具 yaml（`namespace: synth`、`name: <n>`、`version: 0.1.0`、**`maturity: exploratory`**、一个 param `input`(string)、一个 output `result`(Text)、`runtime.command: [/usr/bin/env, python3, <脚本绝对路径>]`）→ `ToolSpec::from_simple_yaml` → `register_tool`。报告「VALIDATED → 注册为 exploratory(低信任)」+ 脚本路径。
   - 不含 → **拒绝**：报告「REJECTED」+ 实际输出 + 脚本路径（供人工查），**不注册**。
   - synthesizer 命令缺失/非零退出 / 脚本运行失败 → 清晰错误，不注册。

## 验收标准（Claude 审核逐条核对）

- [ ] `clippy -D warnings` 无警告；`cargo test` 全绿，净增；现有 49 cli / 180 core 测试不改且通过；core 未改。
- [ ] 无新依赖/表/event_type。
- [ ] **离线 stub 测试**：①stub 输出正确脚本（如读 SYNTH_INPUT 文件并打印其行数/内容）→ 验证通过 → 工具以 `exploratory` 注册（`tools list` 可见）；②stub 输出错误脚本（输出不含 expect）→ **拒绝、未注册**；③synthesizer 缺失 → 错误。
- [ ] 注册的合成工具 maturity == exploratory（断言）。
- [ ] markdown 围栏剥离有测试。
- [ ] grep 确认改动仅 `synth_commands.rs` + lib.rs dispatch/usage；core 未改。

## 不在本里程碑（明确排除，诚实声明）

- 把 LLM 接进循环引擎（ArgumentEngine/BranchSelector 的 LLM 实现）—— 独立后续（同样的后端可配接法）。
- 循环自动触发 synth（agent run 检测 no-tool-match → 自动合成）—— 后续 S2。
- 合成工具证据 grade 封顶强制（exploratory 工具产出不得当 observed）—— 后续 S2。
- 真沙箱（资源/网络隔离）—— 现仅隔离 cwd + 超时；真隔离是已知大缺口。
- 多文件/非 Python 工具、依赖安装。
