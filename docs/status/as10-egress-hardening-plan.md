# AS10 实现简报：诚实出网加固（M1 DNS-pinning + M3 seatbelt 沙箱）

Status: Assigned to Codex（继续 feat/question-aware-fundamental 分支，AS7–AS9 之上新增提交）
Owner: Claude(编排) · Codex(执行)
Spec source: security-reviewer 的 M1（DNS-rebinding/TOCTOU）+ M3（合成工具运行时无出网管控）；编排者裁定为「诚实加固」范围（不做会给假安全感的 raw-socket-不可挡的纯 env 代理）
Depends on: AS1–AS9（本分支已有 6 个提交，core 284 / cli 94 绿，acceptance 通过）

## 背景与边界（必须诚实）

macOS 无 network namespace。**纯 env-proxy 挡不住恶意生成代码的 raw socket**，故本里程碑不做 egress 代理。改为两项「在我们能真正控制的层面做扎实」的加固：

1. **M1（我们自己控制的 probe fetch 路径）**：probe 的 Python 抓取脚本做 DNS-pinning + 解析后 IP 校验，关闭"先校验字符串 host、后由 urllib 二次解析连接"的 TOCTOU 窗口。
2. **M3（生成工具运行时）**：用 macOS `sandbox-exec`(seatbelt) 包住 `run_python_script`，以 **`(allow default)` + 针对性 `deny network-outbound`** 的保守 profile 阻断最高价值 SSRF 目标(loopback 本地服务 + 云元数据 169.254.169.254)。**诚实声明残留**：seatbelt 无法按 CIDR 过滤，RFC1918 内网主机(10/172.16/192.168)在 80/443 上仍可达——这是已记录的残留风险，彻底封堵需 OS 级 host-allowlist 出网（VM/容器/pf），不在本里程碑。

## 编排者裁决（约束）

### 1. [M1] probe fetch 脚本 DNS-pinning + 解析 IP 校验

`crates/agentflow-cli/src/synth_commands.rs`：`SOURCE_PROBE_FETCH_PY`(~51) 与 `CBIOPORTAL_DISCOVERY_FETCH_PY`(~39)（两者已含 AS9 的 `_NoRedirect`）。在两脚本里、发起请求前注入一个**校验型 `socket.getaddrinfo` 包装**：

```python
import socket, ipaddress

_real_getaddrinfo = socket.getaddrinfo

def _is_blocked_ip(ip_str):
    addr = ipaddress.ip_address(ip_str)
    if addr.is_private or addr.is_loopback or addr.is_link_local \
       or addr.is_reserved or addr.is_multicast or addr.is_unspecified:
        return True
    # CGNAT / shared address space (not always covered by is_private)
    if addr.version == 4 and addr in ipaddress.ip_network("100.64.0.0/10"):
        return True
    return False

def _validating_getaddrinfo(host, *args, **kwargs):
    results = _real_getaddrinfo(host, *args, **kwargs)
    for res in results:
        ip_str = res[4][0]
        if _is_blocked_ip(ip_str):
            raise RuntimeError("blocked non-public resolved IP %s for host %s" % (ip_str, host))
    return results

socket.getaddrinfo = _validating_getaddrinfo
```

要点（务必遵守）：
- 该包装必须在 `urllib`/`http.client` 实际连接发生前安装；因为 `http.client` 对一次连接只调用一次 `socket.getaddrinfo`，包装后返回的就是连接将使用的同一批 IP——**单次解析、连接即用**，关闭 TOCTOU。SNI/证书仍用原 hostname（urllib 仍传 hostname），无需改动 TLS。
- 任一解析 IP 命中黑名单即 `raise` → 子进程非 0 退出 → 既有 `fetch_*` 走"失败"分支，trace 记 probe 失败。
- 不改 argv 约定、超时、`MAX_SOURCE_PROBE_BYTES`、`_NoRedirect`。注释标明这是 DNS-rebinding/SSRF 防护。
- 169.254.169.254（云元数据）由 `is_link_local`(169.254.0.0/16) 覆盖；仍保留上面显式 CGNAT 检查与注释点名 metadata。

### 2. [M3] `run_python_script` 用 seatbelt 沙箱包裹（macOS，保守 profile，fail-open 跨平台）

`crates/agentflow-cli/src/synth_commands.rs`：`run_python_script`(~2144) 当前 `Command::new("/usr/bin/env").arg("python3").arg(script_path)`。新增逻辑：

- 新增常量 seatbelt profile 字符串（`(allow default)` 基线，避免破坏 python 启动；末尾针对性 deny，last-match-wins 覆盖）：
  ```
  (version 1)
  (allow default)
  (deny network-outbound (remote ip "localhost:*"))
  (deny network-outbound (remote ip "127.0.0.1:*"))
  (deny network-outbound (remote ip "::1:*"))
  (deny network-outbound (remote ip "169.254.169.254:*"))
  ```
- 新增辅助函数 `fn sandbox_exec_available() -> bool`：返回 `cfg!(target_os = "macos") && Path::new("/usr/bin/sandbox-exec").exists()`。
- 在 `run_python_script` 构造 `Command` 时：
  - 若 `sandbox_exec_available()`：`Command::new("/usr/bin/sandbox-exec")`，args 依次为 `-p`、`<profile>`、`/usr/bin/env`、`python3`、`script_path`；**其余不变**（`env_clear()`、`PATH`/`PYTHONPATH`/`AGENTFLOW_*` env、`current_dir`、stdio、`configure_child_process_group`、`SYNTH_INPUT`、domain-param env 全部照旧设置在这个 Command 上，会经 sandbox-exec 透传给 python）。
  - 否则（非 macOS / 无 sandbox-exec，如 Linux CI）：保持现状 `Command::new("/usr/bin/env").arg("python3")...`。
  - 用一个内部 helper（如 `fn python_command(script_path, workdir) -> Command`）封装"选 sandbox-exec 还是 env"，避免两份重复的 env 设置代码；`run_python_script` 调它再追加 stdio/inputs。
- **fail-open 是有意为之**：seatbelt 是纵深防御第 N 层（主控制是 allowlist-grounded prompt + fixture 验证 + no-fabrication + M1 DNS-pin），且 Linux CI 无 sandbox-exec；若因缺失而 fail-closed 会破坏所有工具执行与 CI。在代码注释与本简报「残留」处说明。

### 3. 不变量与约束

- 不改 `argument.rs` 判决逻辑、不改 `PUBLIC_SOURCE_ALLOWLIST` 内容、不改 `DecisionKind`/事件结构、不改 AS8.1/AS8.2 降级与等价分支逻辑、不改 AS9 的 `_NoRedirect`/probe 预算/缓存。
- 无新 Rust 依赖（用 `std::path::Path` 判存在即可）。
- core 零 LLM/网络依赖不变。

## 测试

- `synth_commands.rs` 单测：
  - `python_probe_scripts_pin_dns_and_reject_private_ips`：断言两个 FETCH_PY 含 `_validating_getaddrinfo`、`socket.getaddrinfo =`、`is_loopback`、`is_link_local`、`100.64.0.0/10`、`raise`。
  - `run_python_script_uses_sandbox_exec_when_available`（macOS-gated，`#[cfg(target_os = "macos")]`）：构造一个最简单的 python 脚本（只 `print` 并写 `AGENTFLOW_OUTPUT_RESULT`），跑 `run_python_script`，断言 `exit_code == Some(0)` 且 result 正确——**证明沙箱不破坏正常执行**。
  - seatbelt profile 常量存在性/格式断言（含四条 deny、`(allow default)`）。
  - 既有 runtime-gate / validation 测试在本机(macOS)经 sandbox-exec 跑仍全绿（关键回归点：合法工具仍能联网命中 cBioPortal）。
- 不要求单测在 CI 里验证"loopback 被拦截"（联网+平台相关）；该项由编排者在本机手动验证（见验收）。

## 交付物

- `synth_commands.rs`：两 FETCH_PY 注入 DNS-pin 校验；新增 seatbelt profile 常量 + `sandbox_exec_available()` + `python_command()` helper；`run_python_script` 接线；上述单测。
- `docs/status/as10-egress-hardening-plan.md`（本文件）。

## 验收标准（Claude 复核 + 本机手验）

- [x] fmt / clippy / core(284) / cli(97，本机真沙箱) / `scripts/acceptance-v1.sh` 全绿；`argument.rs` 仍 0 处 LLM/网络。
- [x] 单测证明两 probe 脚本含 DNS-pin 校验逻辑。
- [x] macOS-gated 单测证明 sandbox-exec 包裹下普通 python 工具仍 `exit_code==0`（不破坏正常执行）。
- [x] **编排者本机手验（macOS，真 sandbox-exec）**：① `urllib`/socket 连 `127.0.0.1`（name+数字）及生成工具连 `127.0.0.1:8080/admin` 均 EPERM 被拒；②连公网 allowlisted host (`www.cbioportal.org`) 仍成功——沙箱选择性生效（见上「本机实证」）。
- [x] core 测试数不减少（284）；无新依赖。

## 实现偏差与本机实证（编排者验证后修正）

简报最初设想 profile 含 `127.0.0.1:*`/`169.254.169.254:*` 等数字 IP deny；本机 macOS `sandbox-exec` 报 `host must be * or localhost in network address`，**数字 IP 行无法通过 profile 解析**。最终 profile 只保留一条 `(deny network-outbound (remote ip "localhost:*"))`。编排者在本机（真 sandbox-exec，`sandbox_exec_usable()=true`）逐项实测，结论如下：

- ✅ **loopback by name 被拦截**：`localhost:22` → EPERM。
- ✅ **loopback 数字也被拦截**：`127.0.0.1:22` → EPERM（seatbelt 的 `localhost` 规则涵盖数字回环，优于 codex 初判）。
- ✅ **生成工具 urllib SSRF 到 `127.0.0.1:8080/admin` → EPERM**（M3 的实际威胁被挡）。
- ✅ **公网 allowlisted egress 仍通**：`urllib` HTTPS 命中 `www.cbioportal.org` 正常返回。
- ⚠️ **关键回归（已修复）**：seatbelt 下 macOS 代理自动探测（`urllib` → SystemConfiguration → `configd` 走 loopback）命中 loopback deny，使**所有** `urllib` 请求 EPERM（含生成工具与可信 cBioPortal 客户端）。codex 因其嵌套沙箱令 `sandbox_exec_usable()=false`、未走沙箱路径而漏检。修复：`python_command` 在沙箱分支注入 `no_proxy=*`/`NO_PROXY=*`，关闭代理探测，公网直连恢复、loopback 仍被挡。已本机四例实测确认。

## 不在本里程碑（残留，记录在案）

- seatbelt 无法 CIDR 过滤：RFC1918 内网主机(10/172.16/192.168) 与 metadata `169.254.169.254` 在 80/443 仍可达（数字 IP 无法写进 profile）；metadata 的 DNS-解析路径由 M1 在 probe 侧覆盖，但生成工具直接按 IP 字面量连 metadata 不被 seatbelt 拦。UDP/其它端口由默认 allow 放行（profile 仅针对 loopback network-outbound）。彻底的 host-allowlist 出网需 OS 级方案（VM/容器/pf/单独用户 + 防火墙），属未来里程碑。
- seatbelt 下强制 `no_proxy=*`：沙箱内放弃系统代理支持，换取确定性直连 allowlisted 公网 host —— 对本项目直连用例可接受。
- 不引入 egress 代理（macOS 上挡不住 raw socket，给假安全感）。
