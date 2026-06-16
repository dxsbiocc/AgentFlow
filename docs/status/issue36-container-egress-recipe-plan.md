# 简报：issue #36 部署级出网封堵配方（容器/netns + egress allowlist，纯 docs/ops）

Status: Assigned to Codex（worktree /tmp/af-issue36，branch feat/issue36-container-recipe，从 main e96091c 起）
Spec source: issue #36 的反篡改残留。PR #49 已落地**合作层** Python 出网 guard（in-process，可被恶意脚本 un-patch）。本任务补**部署级**封堵的可执行配方与文档 —— **纯 docs/ops，不改任何 Rust/工具代码**。

## 背景（既有事实，勿改）

- PR #49：`PYTHON_EGRESS_GUARD_SITECUSTOMIZE`（`crates/agentflow-cli/src/synth_commands.rs`）monkeypatch `socket.getaddrinfo`/`socket.socket.connect`，拦私网/loopback/link-local/metadata(169.254.169.254)/reserved/multicast/unspecified/CGNAT(100.64.0.0/10)，放行公网。**合作层**：工具有完整 Python → 可重新赋值 socket / 删 patch 绕过。
- `docs/status/issue36-egress-guard-plan.md` 已诚实写明：反篡改对手需 OS 沙箱（容器/VM/pf），并给了**文字**指引。本任务把那段指引落成**可执行的最小配方 + 校验脚本**。
- 编排者实证（当前 macOS dev 机）：seatbelt 不能按 CIDR/数字 IP deny；系统 python3 受 SIP（DYLD 被剥）；pf 需 root。→ 真封堵只能在容器/netns/VM 边界做。配方目标平台是 **Linux 部署环境**（CI/服务器/容器），不是 dev 机的 seatbelt。

## 交付物（纯文档 + 脚本，无 Rust 改动）

### 1. 部署文档 `docs/ops/egress-containment.md`（新增）

写清楚**威胁模型分层**与**纵深防御**：
- 第 1 层（已有）：in-process 合作 guard —— 给非恶意脚本/prompt-injection 早失败、可读错误；**不挡反篡改对手**。
- 第 2 层（本文档）：OS 边界 default-deny egress + 公网 allowlist —— 真封堵。
- 给出 **3 套可选配方**，按依赖轻重排序，每套含：what / 适用场景 / 完整可粘贴命令 / 验证方法 / 局限：
  1. **Docker `--network none`**（最小无网基线）：合成工具验证/运行在无网容器里；需公网时见配方 2/3。
  2. **Docker + 受控网桥 + iptables/nftables egress allowlist**：default DROP，仅放行明确公网 HTTPS 目标（cBioPortal/NCBI/EBI/Ensembl/GDC），显式 DROP loopback/RFC1918/link-local/169.254.169.254/CGNAT。给出 nftables 规则样例。
  3. **Linux network namespace + veth + nft**（无 Docker 依赖时）：`ip netns` 起隔离 netns，default-deny egress，allowlist 同上。
- 明确：metadata IP `169.254.169.254`、RFC1918、CGNAT 在所有配方里必须 DROP；allowlist 用域名→解析后 IP 或 L7 代理（如 explicit HTTPS proxy + SNI allowlist），并说明纯 IP allowlist 对 CDN 漂移的局限。
- 收尾：把 in-process guard 定位为 defense-in-depth 的**早失败/可读层**，最终封堵由本层执行；交叉链接 `docs/status/issue36-egress-guard-plan.md` 与（若存在）`docs/CAPABILITIES.md` 的 security-boundary 段。

### 2. 校验脚本 `scripts/verify-egress-policy.sh`（新增，可选执行，不进 CI 必跑）

- 一个**自包含 bash**：在给定隔离环境内跑几条 python 探针，断言：
  - 连 `169.254.169.254:80`、`10.0.0.1`、`127.0.0.1` → **失败/超时**（被封）。
  - 连一个公网 allowlist 目标（如 `https://www.cbioportal.org`，HEAD）→ 成功（若环境无网则跳过并提示）。
- 脚本须**优雅降级**：检测不在容器/netns 时打印"本脚本应在配方 1-3 的隔离环境内运行"并退出 0（不误判 dev 机）。
- 顶部注释写清用法与前置条件；不依赖 root 之外的特殊工具（用 `curl`/`python3`/`timeout`）。

### 3. 关联

- 在 issue #36 正文/或文档里把范围更新为："in-process 合作 guard 已交付(PR #49) + 部署级配方已文档化(本)；issue 可在确认配方后收窄/关闭。" （文档里写明；是否实际 `gh issue` 编辑由编排者后续决定，**Codex 不动 gh**。）

## 不变量（硬约束）

- **零 Rust/工具/core 改动**：`git diff crates/` 必须为空；不碰 examples/。仅新增 `docs/ops/egress-containment.md`、`scripts/verify-egress-policy.sh`（+ 必要的交叉链接编辑，限文档）。
- 配方面向 **Linux 部署**；不声称 macOS seatbelt 能做 CIDR（实证否决）。
- 脚本不实际改机器网络（只读探针 + 断言）；不需要 root 即可"安全空跑/降级"。
- 文档诚实：合作 guard ≠ 反篡改；真封堵=OS 边界。
- 无新依赖。`bash -n scripts/verify-egress-policy.sh` 语法通过；shellcheck（若可用）无 error。

## 验收

- [ ] `docs/ops/egress-containment.md`：3 套配方 + 威胁分层 + metadata/RFC1918/CGNAT 必 DROP + allowlist 局限。
- [ ] `scripts/verify-egress-policy.sh`：探针断言私网/metadata 被封、公网放行；非隔离环境优雅降级；`bash -n` 通过。
- [ ] `git diff crates/` 为空；现有 `cargo test --workspace` / `bash scripts/acceptance-v1.sh` 不受影响（纯加文件）。

## 不在本里程碑

- 不做实际的容器化集成/运行时切换（部署方自行采用配方）。
- 不改 in-process guard（PR #49 已足够作为 defense-in-depth 早失败层）。
