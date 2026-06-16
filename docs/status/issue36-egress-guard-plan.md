# 简报：合成工具运行时出网 guard（issue #36 的可达增量 + 诚实残留）

Status: Assigned to Codex（新分支 feat/egress-guard，从 main 起）
Spec source: issue #36（AS10 残留：RFC1918/metadata 在运行时对生成工具仍可达）
约束实证（编排者，当前 macOS）：seatbelt 不能按数字 IP/CIDR deny（"host must be * or localhost"）；运行时用系统 `/usr/bin/python3`（SIP）→ DYLD_INSERT_LIBRARIES 被剥离；pf 需 root；容器是重依赖。**结论：进程内无法完全封堵反篡改的恶意出网。** 本里程碑做**可达的合作层防护 + 诚实记残留**。

## 范围裁决

- **做**：在工具运行时注入一个 Python 出网 guard（sitecustomize），拦截到**私网/loopback/link-local/metadata(169.254.169.254)/CGNAT** 的连接（域名解析到私网 + IP 字面量直连 两路都拦）。挡住**最现实的 prompt-injection 直连**（注入代码直接连内网/metadata）。
- **诚实声明残留**：guard 是合作层——工具有完整 Python,理论上可 un-patch 绕过;**反篡改对手需 OS 沙箱(容器/VM/pf)**。在 #36 与 docs 写明,并给容器配方指引。
- 仅改 `crates/agentflow-cli`;不改 core/argument.rs/工具库/既有 examples 工件。

## 编排者裁决（实现）

### 1. 出网 guard 常量（synth_commands.rs）

新增 Rust const `PYTHON_EGRESS_GUARD_SITECUSTOMIZE: &str`,内容是一个 sitecustomize.py:
```python
import ipaddress, socket
def _blocked(ip):
    a = ipaddress.ip_address(ip)
    if a.is_private or a.is_loopback or a.is_link_local or a.is_reserved \
       or a.is_multicast or a.is_unspecified:
        return True
    if a.version == 4 and a in ipaddress.ip_network("100.64.0.0/10"):
        return True
    return False
_real_gai = socket.getaddrinfo
def _gai(host, *a, **k):
    res = _real_gai(host, *a, **k)
    for r in res:
        if _blocked(r[4][0]):
            raise OSError("agentflow egress blocked: non-public address %s for host %s" % (r[4][0], host))
    return res
socket.getaddrinfo = _gai
_real_connect = socket.socket.connect
def _connect(self, address):
    try:
        host = address[0]
        ipaddress.ip_address(host)   # only literal IPs (hostnames handled by getaddrinfo guard)
        if _blocked(host):
            raise OSError("agentflow egress blocked: non-public address %s" % host)
    except ValueError:
        pass
    return _real_connect(self, address)
socket.socket.connect = _connect
```
（与 probe 脚本 DNS-pin 同一套私网判定;此处额外拦 IP 字面量直连——正是 metadata 那类。）

### 2. 注入（python_command / run_python_script）

- 把 guard 写成 `sitecustomize.py` 到一个 guard 目录（可用 workdir 下的子目录,或一个临时目录;随运行清理思路同 isolated_workdir）。
- `PYTHONPATH` **前置** 该 guard 目录(再接现有 `cbioportal_pythonpath_value()`),使 Python 启动时自动 import sitecustomize → guard 在工具脚本前生效。
- 对**所有** run_python_script 调用生效（验证 + 运行时）。与既有 `no_proxy=*`(沙箱)叠加:工具直连真实目标 → guard 看到真实 IP,公网放行、私网/metadata 拦截。
- 不改变工具脚本的 argv/`__file__`（sitecustomize 不侵入脚本本体)。

### 3. 不变量

- 仅改 CLI;`git diff crates/agentflow-core` 为空;不碰 argument.rs/工具库/DecisionKind。
- 公网解析/直连放行(不破坏合法 cBioPortal 等公网工具);私网/loopback/link-local/metadata/CGNAT 拦截。
- 无新依赖;guard 用 stdlib(ipaddress/socket)。
- 与 AS10 seatbelt loopback + no_proxy、AS11 代理感知 叠加,不冲突。

### 4. 文档/残留（docs）

- 更新 `docs/design/tool-evolution-engine-design.md` 或 issue #36 关联文档/新增 `docs/status/issue36-egress-guard-plan.md`(本文件)说明:
  - guard 挡住合作层/直连式 prompt-injection 出网;
  - **不**挡反篡改对手(可 un-patch)→ 真封堵需容器/VM/pf;给一段最小容器配方指引(在 netns/容器内跑工具运行时 + egress allowlist)。

## 测试（离线,不联网）

- Rust:断言 `PYTHON_EGRESS_GUARD_SITECUSTOMIZE` 含 `getaddrinfo`/`connect`/`is_private`/`is_link_local`/`100.64.0.0/10`/`agentflow egress blocked`;断言 python_command 把 guard 目录前置进 PYTHONPATH。
- Python 行为(离线):以 guard 在 PYTHONPATH 跑一个脚本:
  - `socket.socket().connect(("169.254.169.254",80))` → 抛 "agentflow egress blocked"(IP 字面量直连被拦,**不实际联网**)。
  - `socket.getaddrinfo("localhost",80)` → 抛(解析到 loopback 被拦)。
  - 对一个解析到公网的名字(或 mock)→ 放行(不抛)。可用一个已知公网 IP 字面量如 `1.1.1.1` 断言 `_blocked` 返回 False(纯函数判定,不连接)。
- 既有 cli 测试 + acceptance 保持绿(合法工具不连私网)。

## 验收

- [ ] fmt/clippy/core(未改)/cli/acceptance 全绿。
- [ ] 单测:guard 拦私网/metadata/loopback、放行公网判定;PYTHONPATH 前置。
- [ ] 文档诚实写明 guard 是合作层 + 容器配方 = 真封堵。
- [ ] 仅改 CLI;无新依赖。

## 落地后的安全边界

本增量注入的 `sitecustomize.py` 是**合作层 guard**：它在合成工具脚本运行前 monkeypatch
`socket.getaddrinfo` 与 `socket.socket.connect`，拦截最常见的直连式 prompt-injection
出网路径，例如 metadata IP、loopback、私网、link-local、reserved、multicast、unspecified
与 CGNAT `100.64.0.0/10`。公网地址保持放行，避免破坏合法公共科学数据源访问。

它**不是反篡改沙箱**。生成脚本仍拥有完整 Python 运行时，理论上可以重新赋值 socket 函数、
删除/替换 `sitecustomize` 中的 patch，或通过其他解释器/原生扩展路径绕过。若威胁模型包含
主动反篡改对手，必须把出网策略下沉到 OS/运行时隔离层。

最小容器/namespace 配方指引：

1. 在容器或 VM 内运行合成工具验证/运行时，禁用默认出网或使用默认拒绝的 egress policy。
2. 只放行明确公网 allowlist（例如 cBioPortal/NCBI/EBI/Ensembl/GDC 的 HTTPS 目标），不要放行
   loopback、RFC1918、link-local、metadata、CGNAT。
3. 使用 `--network none` 作为最小无网基线；需要公网时改为受控网桥 + iptables/nftables/pf
   或 Kubernetes `NetworkPolicy`/CNI egress allowlist。
4. 保留本 Python guard 作为 defense-in-depth：它能给合作脚本和非恶意 prompt-injection 失败
   提供更早、更可读的错误，但最终封堵必须由容器/VM/pf 等边界执行。

部署级配方已独立成文：`docs/ops/egress-containment.md` 给出 Linux Docker
`--network none`、Docker 受控网桥 + nftables allowlist、Linux network namespace +
veth + nftables 三套最小 recipe，并配套 `scripts/verify-egress-policy.sh` 做只读
烟测。issue #36 的范围应按"PR #49 已交付合作层 in-process guard；本文档化
OS 边界封堵 recipe"收窄或关闭，但不要把合作层 guard 描述成反篡改沙箱。

## 不在本里程碑

- 不做容器/VM/pf 集成(真封堵反篡改对手)——文档化为部署级方案。
- 不改 VALIDATION_PATH/python 选择(DYLD 路线已被 SIP 否决,不追)。
