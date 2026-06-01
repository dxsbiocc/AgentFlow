use crate::argument::{ArgumentEngine, InconclusiveKind, RuleBasedEngine, Verdict, VerdictReport};
use crate::branch::{BranchAction, BranchDecision, BranchPolicy, RuleBasedSelector};
use crate::handoff::{Cost, DecisionKind, DecisionPoint, HandoffOption, Risk};
use crate::storage::{EventRecord, ProjectStore, StorageError};

const AGENT_CYCLE_COMPLETED_EVENT: &str = "agent.cycle_completed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CycleOutcome {
    HandedOff,
    Advanced,
    Idle,
}

impl CycleOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HandedOff => "handed_off",
            Self::Advanced => "advanced",
            Self::Idle => "idle",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleReport {
    pub checkpoint_id: String,
    pub provisional_verdicts: Vec<String>,
    pub strong_candidates: Vec<String>,
    pub raised_decisions: Vec<DecisionPoint>,
    pub branch_proposals: Vec<BranchDecision>,
    pub outcome: CycleOutcome,
}

impl CycleReport {
    pub fn to_json(&self) -> String {
        let decisions = self
            .raised_decisions
            .iter()
            .map(DecisionPoint::to_json)
            .collect::<Vec<_>>()
            .join(",");
        let branch_proposals = self
            .branch_proposals
            .iter()
            .map(BranchDecision::to_json)
            .collect::<Vec<_>>()
            .join(",");

        format!(
            concat!(
                "{{",
                "\"schema_version\":\"agentflow.agent_cycle.v0\",",
                "\"checkpoint_id\":\"{}\",",
                "\"provisional_verdicts\":{},",
                "\"strong_candidates\":{},",
                "\"raised_decisions\":[{}],",
                "\"branch_proposals\":[{}],",
                "\"outcome\":\"{}\"",
                "}}"
            ),
            escape_json(&self.checkpoint_id),
            json_string_array(&self.provisional_verdicts),
            json_string_array(&self.strong_candidates),
            decisions,
            branch_proposals,
            self.outcome.as_str()
        )
    }
}

impl ProjectStore {
    pub fn run_cycle(&self) -> Result<CycleReport, StorageError> {
        let checkpoint = self.create_checkpoint("agent_cycle")?;
        let engine = RuleBasedEngine;
        let mut provisional_verdicts = Vec::new();
        let mut strong_candidates = Vec::new();
        let mut raised_decisions = Vec::new();
        let mut branch_proposals = Vec::new();

        for hypothesis in self.list_hypotheses()? {
            let evidence = self.evidence_for(&hypothesis.id)?;
            let preview = engine.render(&hypothesis.id, &evidence);
            match &preview.verdict {
                Verdict::Inconclusive(InconclusiveKind::Provisional { .. }) => {
                    self.render_verdict(&hypothesis.id, &engine, None)?;
                    provisional_verdicts.push(hypothesis.id);
                }
                Verdict::Affirmed
                | Verdict::Refuted
                | Verdict::Inconclusive(InconclusiveKind::Fundamental { .. }) => {
                    let point = self.raise_decision_point(
                        DecisionKind::DeepenOrStop,
                        &strong_verdict_digest(&preview),
                        strong_verdict_options(&preview.hypothesis_id),
                        0,
                    )?;
                    strong_candidates.push(preview.hypothesis_id.clone());
                    raised_decisions.push(point);
                }
            }
        }

        let decisions = self.select_branches(
            &RuleBasedSelector,
            &BranchPolicy {
                explore_enabled: false,
            },
        )?;
        for decision in decisions {
            match &decision.action {
                BranchAction::Abandon {
                    reason,
                    recommend_status,
                } => {
                    let point = self.raise_decision_point(
                        DecisionKind::GoalMutation,
                        &abandon_branch_digest(&decision, reason, *recommend_status),
                        abandon_branch_options(&decision.candidate.hypothesis_id),
                        0,
                    )?;
                    raised_decisions.push(point);
                }
                BranchAction::Deepen { .. } | BranchAction::Spawn { .. } => {
                    branch_proposals.push(decision);
                }
                BranchAction::Hold { .. } => {}
            }
        }

        let outcome = if raised_decisions.is_empty() {
            if provisional_verdicts.is_empty() && branch_proposals.is_empty() {
                CycleOutcome::Idle
            } else {
                CycleOutcome::Advanced
            }
        } else {
            CycleOutcome::HandedOff
        };

        let report = CycleReport {
            checkpoint_id: checkpoint.id,
            provisional_verdicts,
            strong_candidates,
            raised_decisions,
            branch_proposals,
            outcome,
        };
        self.append_event(EventRecord {
            flow_id: None,
            step_id: None,
            run_id: None,
            event_type: AGENT_CYCLE_COMPLETED_EVENT.to_string(),
            payload_json: cycle_completed_payload_json(&report),
        })?;
        self.touch_project()?;

        Ok(report)
    }
}

fn strong_verdict_digest(preview: &VerdictReport) -> String {
    format!(
        "假设 {} 的证据预览为 {}；凭证：{}；支持证据 {}；反证 {}；需人类补防自欺 gate 后才能定论",
        preview.hypothesis_id,
        verdict_label(&preview.verdict),
        preview.rationale,
        evidence_ids(&preview.supporting),
        evidence_ids(&preview.contradicting)
    )
}

fn abandon_branch_digest(
    decision: &BranchDecision,
    reason: &str,
    recommend_status: crate::hypothesis::HypothesisStatus,
) -> String {
    format!(
        "分支 {} 的选择器建议放弃；原因：{}；推荐状态：{}；停止分支属于目标变更，需人类确认",
        decision.candidate.hypothesis_id, reason, recommend_status
    )
}

fn strong_verdict_options(hypothesis_id: &str) -> Vec<HandoffOption> {
    vec![
        HandoffOption {
            label: "确认并补 gate".to_string(),
            direction: format!("为假设 {hypothesis_id} 补齐防自欺 gate 后再定论"),
            cost: Cost::Moderate,
            risk: Risk::Medium,
            reversible: true,
        },
        HandoffOption {
            label: "继续收集证据".to_string(),
            direction: format!("保持假设 {hypothesis_id} 未定论并继续补证据"),
            cost: Cost::Moderate,
            risk: Risk::Low,
            reversible: true,
        },
        HandoffOption {
            label: "放弃该假设".to_string(),
            direction: format!("停止推进假设 {hypothesis_id}"),
            cost: Cost::Cheap,
            risk: Risk::High,
            reversible: false,
        },
    ]
}

fn abandon_branch_options(hypothesis_id: &str) -> Vec<HandoffOption> {
    vec![
        HandoffOption {
            label: "放弃".to_string(),
            direction: format!("确认停止分支 {hypothesis_id}"),
            cost: Cost::Cheap,
            risk: Risk::Medium,
            reversible: false,
        },
        HandoffOption {
            label: "保留".to_string(),
            direction: format!("保留分支 {hypothesis_id} 并暂不变更目标"),
            cost: Cost::Cheap,
            risk: Risk::Low,
            reversible: true,
        },
        HandoffOption {
            label: "再查".to_string(),
            direction: format!("继续调查分支 {hypothesis_id} 再决定是否停止"),
            cost: Cost::Moderate,
            risk: Risk::Low,
            reversible: true,
        },
    ]
}

fn verdict_label(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::Affirmed => "affirmed",
        Verdict::Refuted => "refuted",
        Verdict::Inconclusive(InconclusiveKind::Provisional { .. }) => "inconclusive_provisional",
        Verdict::Inconclusive(InconclusiveKind::Fundamental { .. }) => "inconclusive_fundamental",
    }
}

fn evidence_ids(evidence: &[crate::argument::EvidenceLink]) -> String {
    if evidence.is_empty() {
        "none".to_string()
    } else {
        evidence
            .iter()
            .map(|link| link.id.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn cycle_completed_payload_json(report: &CycleReport) -> String {
    format!(
        concat!(
            "{{",
            "\"checkpoint_id\":\"{}\",",
            "\"provisional_verdict_count\":{},",
            "\"strong_candidate_count\":{},",
            "\"raised_decision_count\":{},",
            "\"branch_proposal_count\":{},",
            "\"outcome\":\"{}\"",
            "}}"
        ),
        escape_json(&report.checkpoint_id),
        report.provisional_verdicts.len(),
        report.strong_candidates.len(),
        report.raised_decisions.len(),
        report.branch_proposals.len(),
        report.outcome.as_str()
    )
}

fn json_string_array(values: &[String]) -> String {
    let items = values
        .iter()
        .map(|value| format!("\"{}\"", escape_json(value)))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
}

fn escape_json(input: &str) -> String {
    let mut output = String::new();
    for ch in input.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            ch if ch.is_control() => output.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => output.push(ch),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rusqlite::params;

    use crate::argument::{EvidenceGrade, EvidenceLinkRequest, Stance};
    use crate::handoff::DecisionKind;
    use crate::hypothesis::HypothesisRequest;
    use crate::storage::{now_unix_seconds, ProjectStore};

    use super::CycleOutcome;

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-agent-{test_name}-{}-{}",
            std::process::id(),
            now_unix_seconds()
        ))
    }

    fn init_project(test_name: &str) -> (PathBuf, ProjectStore) {
        let path = temp_project_path(test_name);
        let _ = std::fs::remove_dir_all(&path);
        let store = ProjectStore::init(&path, Some("Agent Demo")).unwrap();
        (path, store)
    }

    fn record_hypothesis(store: &ProjectStore, statement: &str) -> String {
        store
            .record_hypothesis(HypothesisRequest {
                statement: statement.to_string(),
                origin: "agent test".to_string(),
                related_goal_id: "goal_agent".to_string(),
            })
            .unwrap()
            .id
    }

    fn link_evidence(
        store: &ProjectStore,
        hypothesis_id: &str,
        grade: EvidenceGrade,
        stance: Stance,
        note: &str,
    ) {
        store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: hypothesis_id.to_string(),
                observation_id: None,
                source: None,
                grade,
                stance,
                note: note.to_string(),
            })
            .unwrap();
    }

    fn event_count(store: &ProjectStore, event_type: &str) -> i64 {
        store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM events WHERE event_type = ?1",
                params![event_type],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn weak_evidence_renders_provisional_and_advances() {
        let (path, store) = init_project("weak-provisional");
        let first = record_hypothesis(&store, "Weak support remains provisional");
        let second = record_hypothesis(&store, "Unsupported support remains provisional");
        link_evidence(
            &store,
            &first,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is below the decision margin.",
        );
        link_evidence(
            &store,
            &second,
            EvidenceGrade::Unsupported,
            Stance::Supports,
            "Unsupported support carries no rule weight.",
        );

        let report = store.run_cycle().unwrap();

        assert_eq!(report.outcome, CycleOutcome::Advanced);
        assert_eq!(report.provisional_verdicts, vec![first.clone(), second]);
        assert!(report.strong_candidates.is_empty());
        assert!(report.raised_decisions.is_empty());
        assert_eq!(report.branch_proposals.len(), 2);
        assert_eq!(event_count(&store, "argument.verdict_rendered"), 2);
        assert_eq!(event_count(&store, "agent.cycle_completed"), 1);
        assert!(report.to_json().contains("\"outcome\":\"advanced\""));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn strong_affirmed_preview_raises_gate_decision_without_rendering_verdict() {
        let (path, store) = init_project("strong-affirmed");
        let hypothesis_id = record_hypothesis(&store, "Observed support should be gated");
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::Observed,
            Stance::Supports,
            "Observed support reaches the rule margin.",
        );

        let report = store.run_cycle().unwrap();

        assert_eq!(report.outcome, CycleOutcome::HandedOff);
        assert!(report.provisional_verdicts.is_empty());
        assert_eq!(report.strong_candidates, vec![hypothesis_id.clone()]);
        assert_eq!(report.raised_decisions.len(), 1);
        assert_eq!(report.raised_decisions[0].kind, DecisionKind::DeepenOrStop);
        assert!(report.raised_decisions[0].digest.contains("affirmed"));
        assert!(report.raised_decisions[0].digest.contains("凭证"));
        assert_eq!(event_count(&store, "argument.verdict_rendered"), 0);
        assert!(store.latest_verdict_for(&hypothesis_id).unwrap().is_none());

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn refuted_candidate_raises_abandon_decision_point() {
        let (path, store) = init_project("refuted-abandon");
        let hypothesis_id = record_hypothesis(&store, "Observed contradiction should stop branch");
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::Observed,
            Stance::Contradicts,
            "Observed contradiction reaches the rule margin.",
        );
        store
            .render_verdict(
                &hypothesis_id,
                &crate::argument::RuleBasedEngine,
                Some(gate()),
            )
            .unwrap();

        let report = store.run_cycle().unwrap();

        assert_eq!(report.outcome, CycleOutcome::HandedOff);
        assert_eq!(report.strong_candidates, vec![hypothesis_id]);
        assert!(report
            .raised_decisions
            .iter()
            .any(|point| point.kind == DecisionKind::GoalMutation
                && point.digest.contains("建议放弃")));
        assert_eq!(event_count(&store, "agent.cycle_completed"), 1);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn empty_project_is_idle() {
        let (path, store) = init_project("empty-idle");

        let report = store.run_cycle().unwrap();

        assert_eq!(report.outcome, CycleOutcome::Idle);
        assert!(report.provisional_verdicts.is_empty());
        assert!(report.strong_candidates.is_empty());
        assert!(report.raised_decisions.is_empty());
        assert!(report.branch_proposals.is_empty());
        assert_eq!(event_count(&store, "trace.checkpoint_created"), 1);
        assert_eq!(event_count(&store, "agent.cycle_completed"), 1);

        let _ = std::fs::remove_dir_all(path);
    }

    fn gate() -> crate::argument::SelfDeceptionGate {
        crate::argument::SelfDeceptionGate {
            supports: "Observed contradiction reviewed.".to_string(),
            against: "Potential supporting evidence checked.".to_string(),
            alternatives: "Alternative explanation checked.".to_string(),
            data_quality_risks: "Fixture quality risk noted.".to_string(),
            assumptions: "Fixture represents local evidence.".to_string(),
            falsifier: "Independent support would weaken refutation.".to_string(),
            claim_basis: crate::argument::ClaimBasis::Observed,
            not_yet_claimable: "Not claimable outside this test fixture.".to_string(),
        }
    }
}
