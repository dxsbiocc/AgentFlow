# AS11 实现简报：probe DNS-pin 代理感知（修复 AS10 在代理机器上的联网回归）

Status: Assigned to Codex（新分支 fix/probe-proxy-aware-dns-pin，从 main 起,main 已含 AS7–AS10 合并提交 4eae45e）
Owner: Claude(编排) · Codex(执行)
Spec source: 编排者在 fresh live 案例(STK11/KRAS/NSCLC/sotorasib)中实证定位的 AS10 回归
Depends on: AS7–AS10（已合并入 main）

## 背景（live 实证 + 根因）

合并后用一个**全新领域**假设跑 live（STK11 共突变是否预测 NSCLC 对 sotorasib 的耐药），流程整体正确（工具不过拟合判 low、源发现自主推理出 sotorasib/NSCLC 数据需求、安全 allowlist 跳过 `gdc.cancer.gov`、FundamentalGap 诚实交接、去重稳定、容错），**但所有真实网络 probe 全部 “failed or timed out”**。

根因（已逐项实证）：AS10 给两个 probe 抓取脚本注入了 `_validating_getaddrinfo`（DNS-pin，拒绝私网/loopback IP）。本机（以及任何用本地 HTTP 代理的机器：公司网/VPN/Clash/Surge/mitmproxy 等）设置了 `HTTPS_PROXY=http://127.0.0.1:9981`。urllib 走代理时，`getaddrinfo` 解析的是**代理 host `127.0.0.1`** → DNS-pin 判为 loopback → `RuntimeError: blocked non-public resolved IP 127.0.0.1` → probe 失败。

实证对照：
- `curl -sI https://www.cbioportal.org/api/genes/THRSP` → `200`，无重定向（目标本身可达）。
- 现 probe 脚本（带 DNS-pin）走代理 → 抛 `blocked ... 127.0.0.1`。
- 同脚本 `no_proxy='*'` 直连 → 正常返回 `{"hugoGeneSymbol":"THRSP",...}`。

后果：代理机器上源发现永远连不上任何源 → FundamentalGap 有一部分是“探测连不上”而非“数据不存在”造成的，结论不完全可信。

## 编排者裁决（约束）

**范围只做这一个回归修复，不夹带任何功能。**

### 修复：DNS-pin 豁免已配置的代理 host

`crates/agentflow-cli/src/synth_commands.rs` 的两个常量 `SOURCE_PROBE_FETCH_PY` 与 `CBIOPORTAL_DISCOVERY_FETCH_PY`：在安装 `_validating_getaddrinfo` 之前，解析出当前配置的代理 host 集合；`_validating_getaddrinfo` 内若被解析的 `host` 命中代理集合，直接返回原始解析结果、**跳过私网校验**。

参考实现（两脚本一致；注意各自已有 `import ipaddress/socket/urllib.request`，需补 `urllib.parse`）：

```python
from urllib.parse import urlsplit

def _configured_proxy_hosts():
    hosts = set()
    for scheme, value in urllib.request.getproxies().items():
        if scheme == "no" or not value:
            continue
        target = value if "://" in value else "//" + value
        hostname = urlsplit(target).hostname
        if hostname:
            hosts.add(hostname)
    return hosts

_PROXY_HOSTS = _configured_proxy_hosts()

def _validating_getaddrinfo(host, *args, **kwargs):
    results = _real_getaddrinfo(host, *args, **kwargs)
    # When urllib dials a configured proxy, the proxy (not us) resolves the real
    # target host. Pinning the proxy's frequently-loopback IP is meaningless and
    # would block all proxied egress, so skip validation for the proxy host only.
    if host in _PROXY_HOSTS:
        return results
    for res in results:
        ip_str = res[4][0]
        if _is_blocked_ip(ip_str):
            raise RuntimeError("blocked non-public resolved IP %s for host %s" % (ip_str, host))
    return results
```

要点：
- **无代理配置时行为完全不变**：`_PROXY_HOSTS` 为空集 → 全量 DNS-pin（直连 TOCTOU/rebinding 保护原样保留）。
- 只豁免代理 host 本身；其余所有 host 仍走完整私网校验。
- 不改 `_NoRedirect`、不改 allowlist、不改 Rust 调用方、不改超时/字节上限。
- `getproxies()` 的 `no` 键（no_proxy 列表）必须跳过，不当作代理 host。

### 诚实记账（写入本简报「残留」与代码注释）

代理模式下 DNS-pin 对**目标 host**无保护（目标由代理解析）；此时出网安全依赖 Rust 侧字符串 allowlist + `_NoRedirect`。这是合理取舍：DNS-pin 的 rebinding 保护本就只在直连模式有意义。

## 测试

- `synth_commands.rs` 单测：断言两个 FETCH_PY 均含 `getproxies`、`_configured_proxy_hosts`、`_PROXY_HOSTS`、`if host in _PROXY_HOSTS`、`urlsplit`，且仍保留 `_is_blocked_ip`/`_validating_getaddrinfo`/`_NoRedirect`（防止误删既有防护）。
- 既有 `python_probe_scripts_pin_dns_and_reject_private_ips` 等测试保持绿。
- 实际“代理下 probe 连通 / 无代理下仍拒私网”由编排者本机手验（见验收）。

## 验收标准（Claude 复核 + 本机手验）

- [ ] fmt / clippy / `cargo test -p agentflow-core` / `cargo test -p agentflow-cli` / `scripts/acceptance-v1.sh` 全绿；`argument.rs` 仍 0 处 LLM/网络。
- [ ] 单测证明两脚本含代理豁免逻辑且保留既有 DNS-pin/no-redirect 防护。
- [ ] **本机手验（有 `HTTPS_PROXY=127.0.0.1`）**：① 提取后的 `SOURCE_PROBE_FETCH_PY` 走代理探测 `https://www.cbioportal.org/api/genes/THRSP` 成功返回；② 临时把 host 指向一个解析到私网的目标（或断言无代理路径仍拒 `127.0.0.1` 直连目标）以确认私网拦截未被削弱。
- [ ] **重跑 fresh live 案例（真 DeepSeek+网络，纯 `agent run`，不干预）**：probe 真正连上后，观察源发现是找到可用源走合成、还是**确实**落 FundamentalGap——把“探测连不上”与“数据不存在”区分开。
- [ ] core/cli 测试数不减少；无新依赖。

## 不在本里程碑

- 不做 PubMed 引用接地（原则 A）、不做 notebook 复现展示（原则 B）等 Robin 启发的专业性演进——另立路线图。
- 不改 AS10 的 seatbelt 沙箱 / no_proxy 逻辑（沙箱内生成工具走 `no_proxy=*` 直连，不受本修复影响）。
- 不动 RFC1918/metadata OS 级封堵（已在 issue #36 跟踪）。
