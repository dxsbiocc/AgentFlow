# 简报：堵住 probe 出网两处 SSRF 绕过(SEC-2 proxy/env + SEC-3 mapped-IPv6,均 MEDIUM)

Status: Assigned to Codex（worktree /tmp/af-sec23，branch fix/sec-probe-egress，从 main 起）
来源：发布前安全审计(SEC-2 两个引擎都报；SEC-3 Opus 报)。两处都在 `crates/agentflow-cli/src/synth_commands.rs`，合并一个任务做，避免改同一文件冲突。

## SEC-2(MEDIUM):probe 子进程信任 HTTPS_PROXY + 继承父进程 env

漏洞：系统控制的 probe/cbioportal fetch 子进程用 `Command::new("/usr/bin/env")`（`synth_commands.rs:~1578` 与 `~1612`）**没有 `env_clear()`**，继承父进程全部环境，包括 `HTTP(S)_PROXY` 和任何 `*_API_KEY`。同时 Python 端 `_validating_getaddrinfo`（fetch 脚本内）对**已配置的代理 host 跳过私网校验**。组合利用：`HTTPS_PROXY=http://169.254.169.254:80` + 一个 allowlist 内的 probe URL（如 `https://www.ncbi.nlm.nih.gov/...`）→ urllib 走代理 → 系统控制路径连到 metadata/私网。API key 继承也扩大 blast radius。

修复要求：
- 两个 probe `Command`（probe fetch 与 cbioportal discovery fetch）都加 **`.env_clear()`**，只设必要的 `PATH`（与 `no_proxy=*` 视沙箱需要而定，但**不要**保留 `HTTP(S)_PROXY`）。与既有 validation/`run_step` 的 env 卫生一致。
- Python fetch 脚本里**显式禁用代理**：用 `urllib.request.build_opener(ProxyHandler({}), _NoRedirect, ...)` 或等价方式，确保不读环境代理。这样 DNS-pin 始终对**真实目标 host** 生效。
- 若确需支持本地代理(AS11 场景),改为**显式 opt-in**且对代理 host 套用同一套 public-IP 规则;默认必须是"无代理 + DNS-pin 真实目标"。本次默认走"禁用代理"即可,保留 AS11 已有的 proxy-aware 行为时不得让 metadata/私网代理通过。

## SEC-3(MEDIUM):DNS-pin 漏掉 IPv4-mapped / NAT64 IPv6

漏洞：Python 端私网判定 `_is_blocked_ip`（fetch 脚本，约 `:102-110` 与 `:167-175`）以及运行时/校验注入的 sitecustomize `_blocked`（约 `:40-47`，即 `PYTHON_EGRESS_GUARD_SITECUSTOMIZE`）用 `ipaddress` 判 `is_private/is_link_local/is_reserved`，但对 **IPv4-mapped IPv6**（`::ffff:169.254.169.254`）和 **NAT64**（`64:ff9b::a9fe:a9fe`）这些 v6 形式，`ipaddress` 报 `is_private=False` → 不拦。恶意 DNS 对 allowlist host 返回这种 AAAA → 连到内嵌的 metadata/RFC1918。

修复要求（三处 Python blob 全部同步改：两个 fetch 脚本的 `_is_blocked_ip` + sitecustomize 的 `_blocked`）：
- 判定时，若地址是 v6：先取 `addr.ipv4_mapped`（非 None 则对该 v4 递归套用私网/loopback/link-local/reserved/multicast/unspecified/CGNAT 判定）；
- 额外识别 **NAT64 `64:ff9b::/96`**（以及常见 `64:ff9b:1::/48` 视实现可选），提取低 32 位 v4 后同样递归判定；
- 也处理 `::ffff:0:0/96` 之外的等价写法（用 `ipaddress` 的 `ipv4_mapped` 已覆盖标准映射）。
- 保持现有 v4 判定与公网放行不变(公网 v6 仍放行)。

## 不变量(硬约束)

- 仅改 `crates/agentflow-cli/src/synth_commands.rs`（probe Command env + 三个 Python blob 的 IP 判定）。`git diff crates/agentflow-core` 为空（不碰 core/argument.rs）。
- 不破坏合法公网访问(cBioPortal/NCBI/EBI 等仍可达);不引入新依赖(Python 用 stdlib `ipaddress`)。
- 与 AS9/AS10/AS11 既有 DNS-pin/no-redirect/proxy-aware 叠加,不回归。

## 测试(离线,不联网)

- Rust 单测:断言两个 probe `Command` 设了 `env_clear`/不带 proxy(可断言构造逻辑或常量);断言三个 Python blob 字符串包含 `ipv4_mapped` 与 `64:ff9b`(NAT64)与禁用代理(`ProxyHandler({})`)。
- Python 行为(离线纯函数判定,不实际联网):对 `_is_blocked_ip`/`_blocked` 喂 `::ffff:169.254.169.254`、`::ffff:10.0.0.1`、`64:ff9b::a9fe:a9fe` → 应判 blocked;喂一个公网 v4(如 `1.1.1.1`)与公网 v6 → 应放行。可用内联 python 跑断言,**不发起真实连接**。
- `cargo fmt --all --check`、`cargo clippy -p agentflow-cli --all-targets -- -D warnings`、`cargo test -p agentflow-cli`(相关用例)。**不要** `cargo test --workspace`(控制本机负荷)。

不要 commit。报告:改了哪些行、新增测试、确认 core 未动、合法公网仍放行。
