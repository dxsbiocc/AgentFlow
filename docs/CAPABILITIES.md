# AgentFlow 能力与边界总览

Status: Capability overview
Scope: `docs/status/as1-*` 到 `as20-*`、Plan A、issue36（含部署级 egress 配方）、success-path、`docs/design/tool-evolution-engine-design.md`

## 1. AgentFlow 是什么

AgentFlow 是一个 **CLI-first 的本地自治研究运行时**。它不是论文结论生成器，也不是“让 LLM 直接裁判科学真伪”的系统；它的核心价值在于把研究过程拆成可审计的闭环：

1. 提出或导入假设。
2. 匹配已有工具，并判断工具是否真的能回答问题。
3. 在缺口出现时尝试参数推断、自动流程、自动运行、数据源发现或工具合成。
4. 对真实运行结果生成 observation / evidence。
5. 按证据等级、工具成熟度、参数来源和自欺门做判决。
6. 对不能诚实回答的问题，交出 `StanceAssessment` 或 `FundamentalGap`，由人确认。

一句话概括：

> AgentFlow 的主循环是“假设 -> 工具匹配 / fit -> 真分析 -> 分级证据 -> 判决 / 人在环交接”，并且在每一步保留“不要编造、不要把代理问题当答案”的刹车。

## 2. 诚实性不变量

AgentFlow 的核心卖点不是“自动得出更大胆的结论”，而是**让系统很难把不够格的东西伪装成强结论**。

### 2.1 No-fabrication

自动合成工具必须从真实输入或真实公开源取数。拿不到数据时应失败，而不是输出 default、illustrative、fallback panel 之类的假结果。

已有加固包括：

- 合成 prompt 明确禁止硬编码和编造数值。
- 离线 fixture 冒烟只证明“能跑、形状合理”，不被宣传成科学正确性证明。
- 输入敏感性检查会拒绝输出不随输入变化的候选。
- 运行时门要求用真实参数和真实取数跑过，坏工具不注册。

边界也必须说清楚：这些门能抓住大量自欺和低级编造，但不能数学上证明 LLM 生成工具完全正确。

### 2.2 判决出口确定性

`argument.rs` 是判决出口，设计不变量是 **0 LLM / 0 网络调用**。LLM 可以参与 CLI 侧的工具合成、语义 fit、输出 grounding、cohort 选择等上游环节，但最终 `render_verdict` / evidence scoring / gate 必须只读已落库证据和确定性规则。

这使系统可以同时拥有自治能力和可审计判决：自动化负责提出、运行、暴露缺口；判决层不让 LLM 临场发挥。

### 2.3 Grade-cap

证据等级会被来源诚实地封顶：

- 未验证或自动合成工具产出的结果，不能独立把假设推到 `affirmed`。
- 使用推断参数的步骤，即使用户手动把 evidence link 为 `observed`，也会降级。
- 成功路径要求 `observed` 证据来自 verified 工具，且参数已由人确认。

这条规则防止“自动合成工具跑出一个看似显著的数值 -> 直接 affirm”的自欺路径。

### 2.4 人在环

AgentFlow 的自治默认只推进到“可审决策点”，不会把关键科学判断静默吞掉。

- `StanceAssessment`：工具跑出发现后，把是否支持假设交给人确认。
- `FundamentalGap`：源发现后仍找不到能回答问题的数据时，交给人确认“这可能是真实研究空白”。
- 工具泛化、取代、晋升 verified 等会影响未来行为的治理动作，应走 AS18 的人在环谱系 / 取代门。

### 2.5 Question-aware fit

AgentFlow 区分“主题相关”和“能回答问题”。

例如，生存分析工具可能与某个基因、癌种主题相关，但它不能直接回答“免疫治疗响应预测”这个问题。AS8.1 / AS8.2 把 fit 语义从 topical relevance 推向 question-answering capability：

- 纯关键词匹配升为 Medium 的候选会被 question-aware scorer 复核。
- 若工具答不了具体结论，会降回 Low，让源发现 / 合成 / FundamentalGap 路径可达。
- 等价分支判断也使用同一套降级后的候选，避免“看起来有多个工具”反复触发错误刹车。

### 2.6 输出领域校验

AgentFlow 不只看工具有没有跑完，还要读自己的真实输出。

AS15 的输出 grounding 校验用于发现“右工具 / 右主题 / 错领域”的情况：例如假设问 LUAD，工具报告正文却是 LIHC。此时系统应：

- 拒绝该 finding 作为 evidence。
- 不 raise 正常 stance。
- 记录 `output-domain-mismatch:` apply failure。
- 把 mismatch 暴露给后续工具进化引擎。

这条门的意义是阻止“蒙头用”：输出确实是真算出来的，也不能自动等于“回答了这个假设”。

## 3. 自主源发现与 FundamentalGap

AS7–AS8.2 让 AgentFlow 从“只在已有工具 / cBioPortal 里自娱自乐”推进到问题感知的数据源探索。

流程是：

1. 从假设抽取真正需要的数据，例如 ICB 治疗队列、响应标签、表达数据。
2. 让 LLM 提议公开科学源，但系统只探测 allowlist 内的 http(s) 公共域。
3. 对候选源做 probe，并记录 trace。
4. 判断源是否 **has_required_data**，而不只是“有相关基因 / 相关疾病数据”。
5. 若无 viable 源，不落回代理分析，不编造，raise `FundamentalGap`。

因此，AgentFlow 可以诚实说：

> 我查了哪些源、为什么它们不足以回答这个问题；这可能是研究空白，请人类确认。

这不是失败包装成成功，而是科研自治系统必须具备的停止条件。

## 4. 工具进化引擎

Tool Evolution Engine 的目标是让工具库“随使用收敛变强”，而不是每遇一个新任务就累积一个一次性脚本。

### 4.1 触发：mismatch 检测

AS15 是触发器：输出领域校验发现工具产出的领域与假设不一致，就拒证并记录 `output-domain-mismatch:`。

这个 mismatch 不是单纯错误日志，而是“这个工具可能是同一能力的特化版本”的信号。例如 `survival_assoc` 能做生存关联，但 cohort 被硬编码成 LIHC；遇到 LUAD 时，正确方向不是再造 `survival_assoc_luad`，而是考虑把 cohort 提升为参数。

### 4.2 AS16：能力指纹与可泛化候选

AS16 只看不改：

- 从工具 spec 构造能力指纹：输出类型 + required input 类型。
- 在 mismatch 分支中识别 `GeneralizationCandidate`。
- 找出 I/O 签名兼容的 peers。
- 在 cycle report / CLI 输出中 surface “可泛化候选”。

它不改工具、不注册新版本、不新增 DecisionKind、不引入 LLM 到 core。

### 4.3 AS17–AS17.2：cohort 接地与跨队列验证门

AS17 做验证，不做采纳：

- 对可泛化候选识别 cohort / study 这类变异点。
- 从假设推断目标 cohort。
- 临时用原始 cohort + 新 cohort 重跑 runtime gate。
- 两边都过，报告 `promotable`。
- 任一失败，报告 `rejected` 并给出失败原因。

AS17.1 修复纯 LLM 猜 study id 的问题：先取 cBioPortal 真实 study 列表，再让 LLM 在 shortlist 内选，非法答案不能采纳。

AS17.2 进一步保证 cohort 选择质量：当存在 `pan_can_atlas` study 时，确定性偏好 profile 兼容的 pan-cancer atlas 版本，避免 LLM 合法但次优地选到 legacy study 导致验证超时。

### 4.4 AS18 / AS19 / AS20：谱系 / 取代 / 采纳 / cohort 接线（已落地）

进化引擎的闭环已全部落地在 `main`：**检测(AS16) → 验证(AS17) → 注册候选(AS20) → 人工采纳(AS18)**，并由 AS19 把 cohort 推断接进核心 run 循环。

- **AS18（谱系 / 取代 / 治理门）：** append-only `tool_superseded` 事件 + `supersede_tool`（校验两 ref、拒自我取代）；`agentflow tools supersede <old> --by <new>`。旧特化工具**不删除**，而在 `tool_select` 中降权保留（罚分 + `superseded_by:` 理由），successor 优先 → 谱系可追溯。Methods & Tools 报告展示版本血统。这是**人在环采纳门**。
- **AS19（cohort 接线进核心）：** 核心 `CohortInferer` seam（Noop 默认，0 LLM / 0 网络），工具 param 通过声明式 `infer: cohort`（`ParamInferKind`）opt-in；引擎不硬编码领域字符串。cohort 被推断填入的 param 进 `inferred_param_names`，因此被 grade-cap 降级 —— 推断 cohort 的 run 上不去 `affirmed`。
- **AS20（自动注册泛化候选）：** `promotable` 时确定性派生 `<name>_general`（cohort 提升为显式 `infer: cohort` 参数），以 `exploratory` 成熟度注册，append-only `generalization_candidate_registered` 事件，幂等。**治理红线：绝不自动 supersede** —— 只把 `tools supersede` 命令作为建议输出，采纳留给人工门。`ToolSpec::spec_hash()` 作为 stored-spec 哈希的单一真相源。

换句话说，AS15–AS17.2 证明“看见矛盾、形成候选、验证能否泛化”；AS18–AS20 完成“注册候选、谱系、扬弃、取代”，且采纳始终是人工治理动作。

## 5. 成功路径：Plan A 与 affirmed 门槛

Plan A 的本地生存工具用于证明 AgentFlow 能跑出一条干净的、离线的、真计算的成功路径。它不是靠 cBioPortal 网络长尾证明系统，而是用本地导入的表达表和生存表，跑 median split + log-rank，产出 `marker_report`。

成功路径的判决要求很窄：

1. 工具必须是 verified，或至少在判决规则中满足 verified 工具来源要求。
2. 参数必须由人确认，而不是自动推断后直接当强证据。
3. evidence 必须保持 `observed`，没有被 grade-cap 降到 `inferred`。
4. self-deception gate 必须通过，claim basis 要诚实。
5. 判决 margin 必须达到规则阈值，并且存在 observed support。

因此 `affirmed` 不是“自治跑完就自动成立”。自治可以建议、运行、记录、提示 stance；真正把强证据纳入判决，需要人确认关键参数和立场。

这也是 success-path regression 要锁住的机制：正路径证明 `verified + confirmed params + observed evidence` 可以到 `affirmed`；负路径证明未验证工具或推断参数会被 cap，不能偷渡成功。

## 6. 安全姿态分层

AgentFlow 的安全设计是分层加固，而不是声称单一机制能完全防住恶意代码。

### 6.1 Host allowlist

自主源发现只允许探测公开科学数据域的 allowlist。LLM 提议非 allowlist 域时跳过，不抓取。只允许 http(s)，禁止本地文件、localhost 和内网目标。

### 6.2 Probe DNS-pin 与 no-redirect

AS9 / AS10 加固系统控制的 probe fetch 路径：

- 禁止自动跟随重定向，避免 allowlist 域 open redirect 到 metadata / loopback。
- 对 DNS 解析结果做私网、loopback、link-local、reserved、CGNAT 等校验。
- 单次解析即连接，降低 DNS rebinding / TOCTOU 窗口。

### 6.3 代理感知

AS11 修复代理机器上的误伤：当 urllib 连接的是用户已配置的本地代理时，DNS-pin 不应把代理本身的 `127.0.0.1` 当成目标源拒掉。

边界也很清楚：代理模式下目标 host 由代理解析，DNS-pin 不再保护目标解析；此时安全依赖 Rust 侧字符串 allowlist 和 no-redirect。

### 6.4 Seatbelt loopback 沙箱

AS10 在 macOS 上用 `sandbox-exec` 做生成工具运行时的纵深防御，主要拦 loopback SSRF。实现验证后保留诚实边界：

- loopback by name 和数字回环可被拦。
- 公网 allowlisted 访问仍可通。
- 由于 macOS seatbelt 限制，不能按 CIDR 精确 deny RFC1918 / metadata。
- Linux CI / 非 macOS 环境 fail-open，避免破坏正常工具执行。

### 6.5 运行时出网 guard

issue36 增量给 Python 工具运行时注入 `sitecustomize` guard，monkeypatch `socket.getaddrinfo` 与 `socket.socket.connect`，拦截：

- loopback
- RFC1918 私网
- link-local / metadata `169.254.169.254`
- reserved / multicast / unspecified
- CGNAT `100.64.0.0/10`

这能挡住最现实的合作层 prompt-injection 直连路径，尤其是直接连 metadata 或内网 IP。
当前覆盖面是自动合成工具的 validation 路径，以及已注册 `namespace: synth` 工具的 runtime 执行；用户声明的非 synth 本地工具不被强制注入。

但它不是反篡改沙箱。生成脚本拥有完整 Python 运行时，理论上可 un-patch、替换 socket 函数或走其他解释器 / 原生路径绕过。

### 6.6 执行文件系统 staging / container 硬隔离

P1.3 后，runtime 会把每个 step 声明的 input artifact stage 到该 step 的 workdir：
`workdir/inputs/<port>/<filename>`，并只把这些 staged 路径写入 `inputs.json` 与
`AGENTFLOW_INPUT_*` 环境变量。输出仍从 `workdir/outputs/` 中按声明端口采集回 artifact
store。这让本地组合在接口层保持“只经声明 I/O”。

边界必须诚实：local / conda / micromamba 后端下这是**逻辑 staging**。默认 symlink 可以被
follow 到 artifact store；symlink 不可用时会 copy，因此工具得到的总是 workdir 内路径，但
这还不是反篡改的硬 filesystem sandbox。

P-C.1b 后，`runtime.backend: container` 已落地 hard containment：runtime 构造
`<runner> run --rm --network none -v <workdir>:<workdir> -w <workdir> ... <image> <command>`，
只挂载当前 step workdir，且容器内路径与宿主 workdir 相同，所以 staged input/output 绝对路径
保持有效；`AGENTFLOW_*` 环境变量按运行时实际设置的名字用 `-e NAME` 转发。该路径完成 issue #36
的运行时硬出网封堵与 workdir-only filesystem 隔离；egress allowlist、真实 Docker 集成验证和
资源限额仍留后续。

**容器工具模型(重要边界,诚实声明)**:因为 runtime **只挂载 step workdir**,`backend: container`
的工具**必须把自己的代码/解释器烤进 `image`**——`runtime.command` 引用的是**镜像内**路径,
而不是宿主脚本。这与 local/conda 后端不同:`examples/tools/*.py` 这类**宿主脚本工具不能直接换成
container 后端**(它们的脚本不在 workdir、也不在镜像里,真实 `docker run` 会找不到)。容器工具
应当是自包含镜像(工具代码 + 依赖都在镜像里),只通过挂进来的 workdir 交换声明 I/O。

**多引擎模型(Nextflow 式)**:container 工具的稳定身份是 per-tool 的 `runtime.image`;
引擎与 runner 是 per-run profile 的 HOW,由 `--container-engine docker|podman|singularity|apptainer`
和 `--container-runner <path>` 选择。未显式选择引擎时默认仍是 docker;runner 需要来自
`--container-runner` 或兼容旧工具的 `runtime.runner`,否则运行时报错。引擎不进入 cache key:
同一工具 image/command 在 docker、podman、singularity/apptainer 间切换时,结果身份仍由 image
和声明 I/O 决定,不是由"用哪个 engine 跑"决定。

同一个 image-only 工具可以按 run profile 切换引擎,例如:

```bash
# 本地 Docker/Podman profile;image-only 工具需要 run 级 runner 路径。
agentflow run <flow-id> --container-engine docker --container-runner /usr/bin/docker

# HPC Singularity/Apptainer profile。
agentflow run <flow-id> --container-engine singularity --container-runner /usr/bin/singularity
```

这只是模型示例:实际运行需要主机上有真实 engine,且工具 `image` 引用能被所选 engine 解析。
本仓库当前没有对 Docker/Podman/Singularity/Apptainer 做 live 验证。

**验证状态(诚实)**:容器后端目前**仅由离线 argv 断言测试覆盖**(证明命令行构造正确),
**尚未对真实 Docker daemon 跑过**。真实 daemon 验证(确认 `--network none` 真封网、workdir-only
真隔离、自包含镜像工具能跑通)需要一个 Docker 主机,属后续切片;在那之前,不宣称容器后端已
"生产就绪"的反篡改隔离,只能说命令构造正确、模型设计到位。

### 6.7 部署级出网封堵配方

issue36 的部署级残留已收敛为 `docs/ops/egress-containment.md`：在 Linux 部署环境中，
通过 Docker `--network none`、Docker 受控网桥 + nftables allowlist、或 Linux network
namespace + veth + nftables，把真正的 default-deny egress 策略下沉到 OS 边界。
配方显式 DROP metadata `169.254.169.254`、RFC1918、link-local、loopback、CGNAT，
并只放行明确公网 HTTPS 科学目标。`scripts/verify-egress-policy.sh` 可在隔离环境内做
只读烟测；不在隔离环境时会退出 0，避免把开发机误判为失败。

这层才是反篡改威胁模型下的真实 containment。Python in-process guard 仍保留为
defense-in-depth 的早失败/可读错误层，但不能被包装成 anti-tamper 边界。

## 7. 已知边界与残留

AgentFlow 当前边界应明确写出来：

- **不保证 LLM 生成工具科学正确**：fixture、输入敏感性、运行时门、grounding 能降低自欺，但不能替代专家复核。
- **自动合成工具不能独立 affirm**：未验证工具和推断参数会触发 evidence cap。
- **FundamentalGap 不是系统独断结论**：它是“我们没有找到能回答问题的可访问公开源”的诚实交接，需要人确认。
- **OutputGroundingScorer 依赖 LLM 判断**：它是上游拒证 / 暴露 mismatch 的守门器；判决出口仍保持确定性。
- **采纳是人工治理动作（AS18–AS20 已落地）**：`promotable` 触发 AS20 自动注册一个 `exploratory` 泛化候选，但**绝不自动取代**旧工具；注册新版本后的取代 / 谱系 / verified 晋升仍走 AS18 的人工 `tools supersede` 门。自治负责提出候选，人负责采纳。
- **AS17.x cohort 验证首版偏 cBioPortal / study 参数场景**：它证明 spec 级 cohort 参数化路径，不等于所有领域工具都已可自动泛化。
- **网络安全仍有部署级残留**：Python guard 和 seatbelt 是 defense-in-depth；反篡改对手、raw socket、原生扩展、替换解释器等威胁，需要容器 / VM / pf / Kubernetes NetworkPolicy / CNI egress allowlist 等 OS 级边界。
- **issue36 的核心残留**：真正封堵 RFC1918 / metadata / 私网出网，应在容器或 VM 内运行工具，并使用默认拒绝的 egress policy，只放行明确公网 allowlist。部署级 recipe 已记录在 `docs/ops/egress-containment.md`，但采用和运维集成仍由部署方完成；合作层 guard 不能包装成强沙箱。

## 8. 来源索引

本总览主要依据：

- `docs/status/as1-auto-synth-plan.md` 到 `docs/status/as17.2-cohort-selection-quality-plan.md`
- `docs/status/planA-local-survival-tool.md`
- `docs/status/success-path-regression-plan.md`
- `docs/status/issue36-egress-guard-plan.md`
- `docs/ops/egress-containment.md`
- `docs/design/tool-evolution-engine-design.md`

AS18 / AS19 / AS20 已落地并合并入 `main`（见 `docs/status/as18-*`、`as19-*`、`as20-*`），本文件“工具进化引擎”（§4.4）与“已知边界”（§7）两节已同步更新为采纳门 / cohort 接线 / 自动注册候选的现状。
