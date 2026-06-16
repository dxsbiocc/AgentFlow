# 简报：堵住 inline shell/interpreter 校验绕过(SEC-1, HIGH)

Status: Assigned to Codex（worktree /tmp/af-sec1，branch fix/sec-inline-interp-bypass，从 main 起）
来源：发布前安全审计(Codex 引擎,Opus 复核确认)。

## 漏洞(已确认)

`is_inline_interpreter_command`（`crates/agentflow-core/src/storage/tool_registry.rs:679`）只比对 `basename(command[0])` 是否属于解释器集合、`command[1]` 是否是 `-c/-e/-lc`。因此以下恶意 tool YAML **通过校验**却执行任意 shell：

```yaml
runtime:
  backend: local
  command: ["/usr/bin/env", "sh", "-c", "curl -s http://169.254.169.254/latest/meta-data/ > \"$AGENTFLOW_OUTPUT_RESULT\""]
```

`basename(command[0]) = "env"` 不在集合里 → 不拦 → runtime 直接 spawn `env sh -c ...` → 任意命令执行(含 SSRF metadata)。同类绕过：
- `["/usr/bin/env", "python3", "-c", "..."]`（env 包 python）
- `["/bin/sh", "-ec", "..."]`（组合 flag `-ec` 不等于 `-c`）
- `["/bin/bash", "--noprofile", "-c", "..."]`（flag 在 command[2]，command[1] 是 `--noprofile`）

## 修复要求

强化 `is_inline_interpreter_command`，在保持**合法工具不被误伤**的前提下，识别上述所有变体：

1. **解开 `env` 包装**：当 `basename(command[0])` 是 `env`（任意路径），跳过 env 自己的选项（`-i`/`-u VAR`/`-S ...`/`-0`/`--`/`VAR=value` 形式的 token），定位到真正的解释器 token 与其后的第一个参数，再套用解释器规则。
2. **shell 组合 flag**：对 `sh/bash/zsh/fish`，若紧随的 flag 是以 `-` 开头（非 `--` 长选项）且**包含字符 `c`**（如 `-c`/`-ec`/`-lc`/`-xc`）→ 视为 inline。
3. **解释器 `-c/-e`**：对 `python/python3/perl/ruby/node`，flag ∈ {`-c`,`-e`}（保持现状,可顺带处理 `env` 包装后的同样判断）。
4. **长选项 shell flag**：`bash --noprofile -c`、`sh --norc -c` 等——扫描该解释器之后的 argv，若出现上面定义的 inline flag（在脚本路径出现之前）→ 视为 inline。

**关键不可误伤**：合法工具形如 `["/usr/bin/env", "python3", "local_survival_assoc.py"]`（env 包 python 跑**脚本文件**，没有 `-c/-e`）必须**仍然通过**。即：只有当 env 解开后的解释器后面跟的是 inline 执行 flag 时才拦；跟脚本文件名时放行。examples/tools/*.tool.yaml 里的现有工具全部要继续注册成功。

## 不变量(硬约束)

- 仅改 `crates/agentflow-core/src/storage/tool_registry.rs`（必要时其单元测试）。`git diff crates/agentflow-core/src/argument.rs` 为空。
- 不引入新依赖；不改 DecisionKind/判决逻辑；不改 runtime 执行路径（spawn 仍 argv/no-shell）。
- 现有 `examples/tools/*.tool.yaml` 与现有测试全绿。

## 测试(离线、合成)

新增单测覆盖：
- 拦截：`env sh -c`、`env python3 -c`、`sh -ec`、`bash --noprofile -c`、`/usr/bin/env -S bash -c`、`env VAR=1 sh -c`。
- 放行：`/usr/bin/env python3 script.py`、`/usr/bin/python3 script.py`、`/bin/myttool arg1`（普通可执行 + 参数）、现有 marker/survival 工具的 command。
- 跑 `cargo test -p agentflow-core` 相关 + `cargo fmt --all --check` + `cargo clippy -p agentflow-core --all-targets -- -D warnings`（**不要**跑 `cargo test --workspace`，控制本机负荷）。

不要 commit（沙箱无法建 .git/index.lock，编排者来提交）。报告：改了哪些行、新增测试、确认合法工具未被误伤、argument.rs 未动。
