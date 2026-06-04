# 深度审计报告 2026-06-04（main @ 9e92b45）

Owner: Claude(编排) 独立审计 · 方法：代码级不变量核查 + 真实自主链 live 驱动（claude + cBioPortal）

## ✅ 通过项（核心防线坚固）
- 判决落库单一咽喉：`argument.verdict_rendered` 仅 render_verdict(argument.rs:815) 写；branch/forage/report 全调用它，无绕过。
- 判决纯确定性：argument 生产码零 LLM 缝；按 created_at ASC,id ASC 排序可重现。结构性保证 LLM 永不决定判决。
- 自欺闸门随每个判决落库（payload 含 gate）。
- 生产零 panic（4 个全在测试）；report.rs 122 unwrap 全是 writeln!→String（不可失败）；引擎 serde expect 不可失败。
- 控制引擎零吞错（agent/argument/forage/handoff/trace_guard 生产码 0 个 let _ =）。
- PV1 在线生效：推断 gene=THRSP 并通过 pattern 校验。

## 🟠 A1：无 --flow 时自治半途而废（主发现）✅ 已修（AF1, 2026-06-04）
现象：无关联 flow 的新假设跑 `--apply --auto-run`，循环正确起草步骤（gene=THRSP），但 apply 要求 config.flow（agent.rs:300），步骤被静默丢弃——不 apply/不 run/不 raise 决策，却报 outcome=advanced（仅 lifecycle 转 under_test）。无证据产生，看似有进展。
违反：A1（默认自治）+ A4（可见）。是原 🔴 结果惰性在「无 flow 路径」上的较轻回声。
修法候选：
  (a) 无目标 flow 时循环自动建 flow 承载该步（最自治，贴 §1.5 动态图默认自主；人类决策仍发生在 L4 stance 解读处）。
  (b) raise 决策点告知「已起草分析，需 flow 目标/批准」（最保守可见，但在常见路径重新引入闸门，与 §1.5「自动化推进是主线、推翻默认审批」张力）。
状态：✅ 已修。用户选 (a)。AF1 实现自动建 flow 跑完；live 复跑实证：无 --flow → 自动建 auto_<hyp> → 真跑 TCGA(THRSP score -1.165) → StanceAssessment（含 PV2 ⚠）；outcome advanced→handed_off。见 af1-auto-flow-plan.md。

## 🟡 A2：工具匹配仅靠关键词
真正相关的 tcga 工具对 THRSP 假设只匹配 fit=low（纯关键词重叠）。语义相关性未捕捉，削弱 exploit/explore 选择质量。质量局限，非缺陷。后续可选：语义/embedding 匹配或工具声明关键词扩展。

## 🟢 A3：缺 flow list 命令（UX）
有 validate/approve/inspect，无法列出 flows，可发现性小缺口。

## 结论
核心不变量全部坚固。唯一值得修的是 🟠 A1（自治承诺与无-flow 前置之间的缝，落在最常见路径）。A2/A3 为质量/UX 改进，非缺陷。
