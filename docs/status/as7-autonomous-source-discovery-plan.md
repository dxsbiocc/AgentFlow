# AS7 实现简报：自主数据源发现（跳出 cBioPortal,LLM 提议公开源→探测→合成）

Status: Assigned to Codex
Owner: Claude(编排) · Codex(执行)
Spec source: 用户「不能限定在已有工具/数据里自娱自乐,没有现成数据时应自主上网检索、学习获取免疫相关数据」+ 选定「LLM 提议公开科学源+系统探测」
Depends on: AS1-AS6(合成+测试-修复+grounding+验证客户端,已合并 main)

## 背景
当前合成只面向 cBioPortal(grounding/客户端都是它)。但 cBioPortal 无 ICB 免疫治疗响应数据,真答 MID1IP1 免疫治疗问题需跳出去找别的公开源(GEO/ICB队列等)。本里程碑:缺口时让 LLM 提议候选公开源→系统探测可用性→对可用源做 grounded 合成(测试-修复)→数据真不够则诚实呈现(研究空白)。

## 安全边界（必须,自主抓网必带）
- **域名 allowlist**:只探测/抓取已知公开科学数据域(www.cbioportal.org, eutils.ncbi.nlm.nih.gov, www.ncbi.nlm.nih.gov, www.ebi.ac.uk, rest.ensembl.org, api.gdc.cancer.gov 等,可扩展常量表)。LLM 提议的源若不在 allowlist→跳过(不抓)。
- 仅 http(s);禁止 localhost/内网 IP/file://;带超时;探测只读 GET 发现端点。
- 全程可见(探测了哪些源、结果)记入 cycle 输出/日志。

## 编排者裁决（约束）
1. **源提议(LLM)**:缺口时构 prompt 让 LLM 提议候选公开数据源,每个含:name、base_url(域须在 allowlist)、probe_url(一个只读 GET 发现/查询端点,用于验证该源对本假设实体有数据)、access_note。返回排序候选列表(JSON)。
2. **探测(系统)**:对每个候选,allowlist 校验通过后 GET probe_url(超时);若返回非空、plausible(含实体相关数据)→标记 viable,保留探测摘要。全部不可用→无 viable 源。
3. **grounded 合成**:对首个 viable 源——若是 cBioPortal→复用 AS6 验证客户端;否则把 {源信息+probe 摘要} 注入合成 prompt 做从零合成(AS4 测试-修复 + AS5 grounding 思路),运行时门用真数据验。
4. **诚实「无源」**:所有候选不可用 或 合成均失败 → 不编造;在 L4/决策 digest 明示「未找到可访问的公开数据源能为该假设提供数据,可能是真实研究空白(Fundamental)」,交人类。(完整 Fundamental 判决落库可留 AS8;本步至少把「无可用源/研究空白」诚实 surface。)
5. **保留 AS1-AS6 全部**:no-fabrication、输入敏感性、运行时门、测试-修复、L4 ⚠ 未验证、0 判决权重、默认全开、可配置 LLM、dedup、cBioPortal 客户端复用。
6. core 零 LLM 依赖;判决确定性(不被网络数据污染);无新 Rust 依赖(发现/探测走已有 python/urllib 子进程或 std)。

## 交付物
- 新发现模块/synth_commands.rs：源提议 prompt + 解析候选 JSON;allowlist 常量 + 校验;探测(GET probe_url 带超时);viable 选择;注入合成 prompt;无源→诚实 digest。
- agent.rs：缺口→源发现→合成(viable 源)→既有 auto-flow/run/L4;无源→诚实交接 digest。
- 测试(离线 stub,不真联网):mock 候选 + mock 探测响应→选 viable→注入 prompt;allowlist 拦非法域;无 viable→诚实 digest;AS1-AS6 不回归。

## 验收标准（Claude 复核 + live）
- [ ] clippy/test/acceptance/fmt 全绿;AS1-AS6 加固与测试保持。
- [ ] allowlist:非法域候选被拦不抓(单测);仅 http(s)、禁内网。
- [ ] 源提议+探测+viable 选择 单测(mock);无 viable→诚实 digest 含「研究空白」。
- [ ] **live(编排者,真 DeepSeek+网络)**:纯 agent run MID1IP1 免疫治疗 → LLM 提议公开源(可能含 GEO/cBioPortal)→ 系统探测 → 对 viable 源 grounded 合成 → 若某源真能取到相关数据则真跑出结果(L4 ⚠ 未验证);若都不行则诚实呈现「无可用 ICB 数据源=研究空白」,不编造。两种都算通过(关键:跳出了 cBioPortal 去找、全程不编造、诚实结论)。
- [ ] core 无新依赖;判决确定性;安全边界生效。

## 不在本里程碑
- 完整 Fundamental 判决落库(AS8);通用搜索引擎 API(需 key);任意源的可复用客户端库治理;硬安全沙箱(allowlist+超时+只读 GET 为本步边界)。
