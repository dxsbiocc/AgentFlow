use crate::argument::VerdictTag;
use crate::graph_patch::GraphPatchRecord;
use crate::hypothesis::{Confidence, HypothesisStatus};
use crate::storage::{ProjectStore, StorageError};

const SPAWN_SCORE: i32 = 40;
const DEEPEN_SCORE: i32 = 30;
const ABANDON_SCORE: i32 = 10;
const HOLD_SCORE: i32 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CandidateKind {
    Deepen,
    Spawn,
    Abandon,
    Hold,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchCandidate {
    pub hypothesis_id: String,
    pub statement: String,
    pub verdict: Option<VerdictTag>,
    pub confidence: Option<Confidence>,
    pub kind: CandidateKind,
    pub evidence_count: usize,
    pub score: i32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BranchPolicy {
    pub explore_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectionMode {
    Exploit,
    Explore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchAction {
    Deepen {
        reason: String,
    },
    Spawn {
        reason: String,
    },
    Abandon {
        reason: String,
        recommend_status: HypothesisStatus,
    },
    Hold {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchDecision {
    pub candidate: BranchCandidate,
    pub action: BranchAction,
    pub selected_by: SelectionMode,
}

impl BranchCandidate {
    pub fn to_json(&self) -> String {
        format!(
            concat!(
                "{{",
                "\"hypothesis_id\":\"{}\",",
                "\"statement\":\"{}\",",
                "\"verdict\":{},",
                "\"confidence\":{},",
                "\"kind\":\"{}\",",
                "\"evidence_count\":{},",
                "\"score\":{}",
                "}}"
            ),
            escape_json(&self.hypothesis_id),
            escape_json(&self.statement),
            self.verdict
                .map(|verdict| format!("\"{}\"", verdict.as_str()))
                .unwrap_or_else(|| "null".to_string()),
            self.confidence
                .map(|confidence| format!("\"{}\"", confidence.as_str()))
                .unwrap_or_else(|| "null".to_string()),
            candidate_kind_as_str(self.kind),
            self.evidence_count,
            self.score
        )
    }
}

impl BranchDecision {
    pub fn to_json(&self) -> String {
        format!(
            "{{\"candidate\":{},\"action\":{},\"selected_by\":\"{}\"}}",
            self.candidate.to_json(),
            branch_action_json(&self.action),
            selection_mode_as_str(self.selected_by)
        )
    }
}

pub trait BranchSelector {
    fn rank(&self, candidates: Vec<BranchCandidate>) -> Vec<BranchCandidate>;
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RuleBasedSelector;

impl BranchSelector for RuleBasedSelector {
    fn rank(&self, mut candidates: Vec<BranchCandidate>) -> Vec<BranchCandidate> {
        candidates.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.hypothesis_id.cmp(&right.hypothesis_id))
        });
        candidates
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedStep {
    pub id: String,
    pub tool: String,
    pub needs: Vec<String>,
    pub inputs: Vec<(String, String)>,
    pub params: Vec<(String, String)>,
    pub outputs: Vec<(String, String)>,
}

impl ProjectStore {
    pub fn branch_candidates(&self) -> Result<Vec<BranchCandidate>, StorageError> {
        let hypotheses = self.list_hypotheses()?;
        let mut candidates = Vec::with_capacity(hypotheses.len());
        for hypothesis in hypotheses {
            let verdict = self.latest_verdict_for(&hypothesis.id)?;
            let tag = verdict.as_ref().map(|summary| summary.tag);
            let confidence = verdict.as_ref().map(|summary| summary.confidence);
            let kind = kind_for(tag);
            let evidence_count = self.evidence_for(&hypothesis.id)?.len();
            candidates.push(BranchCandidate {
                hypothesis_id: hypothesis.id,
                statement: hypothesis.statement,
                verdict: tag,
                confidence,
                kind,
                evidence_count,
                score: score_for(kind, confidence),
            });
        }
        Ok(candidates)
    }

    pub fn select_branches(
        &self,
        selector: &dyn BranchSelector,
        policy: &BranchPolicy,
    ) -> Result<Vec<BranchDecision>, StorageError> {
        let candidates = self.branch_candidates()?;
        let explore_id = if policy.explore_enabled {
            least_explored_candidate_id(&candidates)
        } else {
            None
        };
        let mut ranked = selector.rank(candidates);
        if let Some(id) = explore_id.as_deref() {
            if let Some(index) = ranked
                .iter()
                .position(|candidate| candidate.hypothesis_id == id)
            {
                let candidate = ranked.remove(index);
                ranked.insert(0, candidate);
            }
        }

        ranked
            .into_iter()
            .map(|candidate| {
                let selected_by = if explore_id.as_deref() == Some(candidate.hypothesis_id.as_str())
                {
                    SelectionMode::Explore
                } else {
                    SelectionMode::Exploit
                };
                let action = action_for(&candidate);
                Ok(BranchDecision {
                    candidate,
                    action,
                    selected_by,
                })
            })
            .collect()
    }

    pub fn propose_branch_patch(
        &self,
        flow_id: &str,
        decision: &BranchDecision,
        step: &ProposedStep,
    ) -> Result<GraphPatchRecord, StorageError> {
        let branch_kind = match &decision.action {
            BranchAction::Deepen { .. } => "deepen",
            BranchAction::Spawn { .. } => "spawn",
            BranchAction::Abandon { .. } | BranchAction::Hold { .. } => {
                return Err(StorageError::InvalidInput(
                    "abandon/hold 不产出图变更：abandon 是需用户决策的建议".to_string(),
                ));
            }
        };
        validate_proposed_step(step)?;

        let title = format!("branch:{branch_kind} {}", decision.candidate.hypothesis_id);
        let reason = action_reason(&decision.action);
        let patch_json = patch_json_for(step);
        self.propose_graph_patch(flow_id, &title, reason, &patch_json)
    }
}

fn least_explored_candidate_id(candidates: &[BranchCandidate]) -> Option<String> {
    let mut selected = None::<&BranchCandidate>;
    for candidate in candidates {
        if selected
            .map(|current| candidate.evidence_count < current.evidence_count)
            .unwrap_or(true)
        {
            selected = Some(candidate);
        }
    }
    selected.map(|candidate| candidate.hypothesis_id.clone())
}

fn kind_for(verdict: Option<VerdictTag>) -> CandidateKind {
    match verdict {
        Some(VerdictTag::Affirmed) => CandidateKind::Spawn,
        Some(VerdictTag::Refuted) => CandidateKind::Abandon,
        Some(VerdictTag::InconclusiveProvisional) => CandidateKind::Deepen,
        Some(VerdictTag::InconclusiveFundamental) => CandidateKind::Abandon,
        None => CandidateKind::Hold,
    }
}

fn score_for(kind: CandidateKind, confidence: Option<Confidence>) -> i32 {
    let base = match kind {
        CandidateKind::Spawn => SPAWN_SCORE,
        CandidateKind::Deepen => DEEPEN_SCORE,
        CandidateKind::Abandon => ABANDON_SCORE,
        CandidateKind::Hold => HOLD_SCORE,
    };
    base + confidence_bonus(confidence)
}

fn confidence_bonus(confidence: Option<Confidence>) -> i32 {
    match confidence {
        Some(Confidence::High) => 6,
        Some(Confidence::Medium) => 3,
        Some(Confidence::Low) => 1,
        None => 0,
    }
}

fn action_for(candidate: &BranchCandidate) -> BranchAction {
    match candidate.kind {
        CandidateKind::Deepen => BranchAction::Deepen {
            reason: "provisional verdict needs more evidence".to_string(),
        },
        CandidateKind::Spawn => BranchAction::Spawn {
            reason: "affirmed verdict supports spawning a related branch".to_string(),
        },
        CandidateKind::Abandon => {
            let (reason, recommend_status) = match candidate.verdict {
                Some(VerdictTag::Refuted) => (
                    "refuted verdict suggests stopping this branch",
                    HypothesisStatus::Contradicted,
                ),
                Some(VerdictTag::InconclusiveFundamental) => (
                    "fundamental inconclusive verdict marks a research frontier",
                    HypothesisStatus::Superseded,
                ),
                _ => (
                    "branch is not actionable without a matching abandon verdict",
                    HypothesisStatus::Superseded,
                ),
            };
            BranchAction::Abandon {
                reason: reason.to_string(),
                recommend_status,
            }
        }
        CandidateKind::Hold => BranchAction::Hold {
            reason: "no verdict is available; render a verdict before branching".to_string(),
        },
    }
}

fn action_reason(action: &BranchAction) -> &str {
    match action {
        BranchAction::Deepen { reason }
        | BranchAction::Spawn { reason }
        | BranchAction::Abandon { reason, .. }
        | BranchAction::Hold { reason } => reason,
    }
}

fn candidate_kind_as_str(kind: CandidateKind) -> &'static str {
    match kind {
        CandidateKind::Deepen => "deepen",
        CandidateKind::Spawn => "spawn",
        CandidateKind::Abandon => "abandon",
        CandidateKind::Hold => "hold",
    }
}

fn selection_mode_as_str(mode: SelectionMode) -> &'static str {
    match mode {
        SelectionMode::Exploit => "exploit",
        SelectionMode::Explore => "explore",
    }
}

fn branch_action_json(action: &BranchAction) -> String {
    match action {
        BranchAction::Deepen { reason } => format!(
            "{{\"kind\":\"deepen\",\"reason\":\"{}\"}}",
            escape_json(reason)
        ),
        BranchAction::Spawn { reason } => format!(
            "{{\"kind\":\"spawn\",\"reason\":\"{}\"}}",
            escape_json(reason)
        ),
        BranchAction::Abandon {
            reason,
            recommend_status,
        } => format!(
            concat!(
                "{{",
                "\"kind\":\"abandon\",",
                "\"reason\":\"{}\",",
                "\"recommend_status\":\"{}\"",
                "}}"
            ),
            escape_json(reason),
            recommend_status.as_str()
        ),
        BranchAction::Hold { reason } => {
            format!(
                "{{\"kind\":\"hold\",\"reason\":\"{}\"}}",
                escape_json(reason)
            )
        }
    }
}

fn validate_proposed_step(step: &ProposedStep) -> Result<(), StorageError> {
    validate_non_empty("branch step id", &step.id)?;
    validate_non_empty("branch step tool", &step.tool)?;
    for need in &step.needs {
        validate_non_empty("branch step need", need)?;
    }
    validate_pairs("branch step input", &step.inputs)?;
    validate_pairs("branch step param", &step.params)?;
    validate_pairs("branch step output", &step.outputs)?;
    Ok(())
}

fn validate_pairs(label: &str, pairs: &[(String, String)]) -> Result<(), StorageError> {
    for (key, value) in pairs {
        validate_non_empty(&format!("{label} key"), key)?;
        validate_non_empty(&format!("{label} value"), value)?;
    }
    Ok(())
}

fn validate_non_empty(label: &str, value: &str) -> Result<(), StorageError> {
    if value.trim().is_empty() {
        Err(StorageError::InvalidInput(format!(
            "{label} must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn patch_json_for(step: &ProposedStep) -> String {
    format!(
        concat!(
            "{{\"ops\":[{{",
            "\"op\":\"add_step\",",
            "\"id\":\"{}\",",
            "\"tool\":\"{}\",",
            "\"needs\":{},",
            "\"inputs\":{},",
            "\"params\":{},",
            "\"outputs\":{}",
            "}}]}}"
        ),
        escape_json(step.id.trim()),
        escape_json(step.tool.trim()),
        string_array_json(&step.needs),
        pairs_json(&step.inputs),
        pairs_json(&step.params),
        pairs_json(&step.outputs)
    )
}

fn string_array_json(values: &[String]) -> String {
    let items = values
        .iter()
        .map(|value| format!("\"{}\"", escape_json(value.trim())))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
}

fn pairs_json(pairs: &[(String, String)]) -> String {
    let fields = pairs
        .iter()
        .map(|(key, value)| {
            format!(
                "\"{}\":\"{}\"",
                escape_json(key.trim()),
                escape_json(value.trim())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{fields}}}")
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

    use crate::argument::{
        ArgumentEngine, ClaimBasis, EvidenceGrade, EvidenceLink, EvidenceLinkRequest,
        InconclusiveKind, SelfDeceptionGate, Stance, Verdict, VerdictReport,
    };
    use crate::hypothesis::{Confidence, Hypothesis, HypothesisRequest};
    use crate::storage::{now_unix_seconds, ProjectStore};

    use super::{
        BranchAction, BranchCandidate, BranchPolicy, BranchSelector, CandidateKind, ProposedStep,
        RuleBasedSelector, SelectionMode,
    };

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-branch-{test_name}-{}-{}",
            std::process::id(),
            now_unix_seconds()
        ))
    }

    #[derive(Debug, Clone)]
    struct FixedEngine {
        verdict: Verdict,
        confidence: Confidence,
    }

    impl ArgumentEngine for FixedEngine {
        fn render(&self, hypothesis_id: &str, _evidence: &[EvidenceLink]) -> VerdictReport {
            VerdictReport {
                hypothesis_id: hypothesis_id.to_string(),
                verdict: self.verdict.clone(),
                confidence: self.confidence,
                supporting: Vec::new(),
                contradicting: Vec::new(),
                rationale: "fixed test verdict".to_string(),
            }
        }
    }

    fn record_hypothesis(store: &ProjectStore, statement: &str) -> Hypothesis {
        store
            .record_hypothesis(HypothesisRequest {
                statement: statement.to_string(),
                origin: "agent".to_string(),
                related_goal_id: "goal_branch".to_string(),
            })
            .unwrap()
    }

    fn record_with_verdict(
        store: &ProjectStore,
        statement: &str,
        verdict: Verdict,
        confidence: Confidence,
    ) -> Hypothesis {
        let hypothesis = record_hypothesis(store, statement);
        let gate = self_deception_gate_for(&verdict);
        store
            .render_verdict(
                &hypothesis.id,
                &FixedEngine {
                    verdict,
                    confidence,
                },
                gate,
            )
            .unwrap();
        hypothesis
    }

    fn self_deception_gate_for(verdict: &Verdict) -> Option<SelfDeceptionGate> {
        match verdict {
            Verdict::Inconclusive(InconclusiveKind::Provisional { .. }) => None,
            Verdict::Affirmed
            | Verdict::Refuted
            | Verdict::Inconclusive(InconclusiveKind::Fundamental { .. }) => {
                Some(SelfDeceptionGate {
                    supports: "Branch test support".to_string(),
                    against: "Branch test contradiction checked".to_string(),
                    alternatives: "Branch test alternative checked".to_string(),
                    data_quality_risks: "Branch test data quality risk".to_string(),
                    assumptions: "Branch test assumption".to_string(),
                    falsifier: "Branch test falsifier".to_string(),
                    claim_basis: ClaimBasis::Observed,
                    not_yet_claimable: "Branch test limitation".to_string(),
                })
            }
        }
    }

    fn add_evidence(store: &ProjectStore, hypothesis_id: &str, note: &str) {
        store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: hypothesis_id.to_string(),
                observation_id: None,
                source: None,
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: note.to_string(),
            })
            .unwrap();
    }

    fn seed_flow(store: &ProjectStore, flow_id: &str) {
        let now = now_unix_seconds();
        store
            .connection()
            .execute(
                "INSERT INTO flows
                 (id, name, status, source_path, schema_version, created_at, updated_at)
                 VALUES (?1, ?2, 'approved', NULL, ?3, ?4, ?5)",
                params![
                    flow_id,
                    format!("Flow {flow_id}"),
                    agentflow_schemas::FLOW_SCHEMA_V0,
                    now,
                    now
                ],
            )
            .unwrap();
    }

    fn candidate(id: &str, kind: CandidateKind, confidence: Option<Confidence>) -> BranchCandidate {
        BranchCandidate {
            hypothesis_id: id.to_string(),
            statement: format!("statement {id}"),
            verdict: None,
            confidence,
            kind,
            evidence_count: 0,
            score: super::score_for(kind, confidence),
        }
    }

    fn proposed_step() -> ProposedStep {
        ProposedStep {
            id: "branch_scan".to_string(),
            tool: "local/scan".to_string(),
            needs: vec!["scan".to_string()],
            inputs: vec![("table".to_string(), "artifact_1".to_string())],
            params: vec![("gene".to_string(), "TP53".to_string())],
            outputs: vec![("report".to_string(), "branch_report".to_string())],
        }
    }

    #[test]
    fn maps_verdicts_to_candidate_kinds_and_hold_without_verdict() {
        let path = temp_project_path("mapping");
        let store = ProjectStore::init(&path, Some("Branch Demo")).unwrap();
        let affirmed = record_with_verdict(
            &store,
            "Affirmed branch",
            Verdict::Affirmed,
            Confidence::High,
        );
        let refuted = record_with_verdict(
            &store,
            "Refuted branch",
            Verdict::Refuted,
            Confidence::Medium,
        );
        let provisional = record_with_verdict(
            &store,
            "Provisional branch",
            Verdict::Inconclusive(InconclusiveKind::Provisional {
                missing: vec!["more evidence".to_string()],
            }),
            Confidence::Low,
        );
        let fundamental = record_with_verdict(
            &store,
            "Fundamental branch",
            Verdict::Inconclusive(InconclusiveKind::Fundamental {
                frontier: "external replication".to_string(),
            }),
            Confidence::Low,
        );
        let hold = record_hypothesis(&store, "No verdict branch");

        let candidates = store.branch_candidates().unwrap();
        assert_eq!(
            candidate_kind(&candidates, &affirmed.id),
            Some(CandidateKind::Spawn)
        );
        assert_eq!(
            candidate_kind(&candidates, &refuted.id),
            Some(CandidateKind::Abandon)
        );
        assert_eq!(
            candidate_kind(&candidates, &provisional.id),
            Some(CandidateKind::Deepen)
        );
        assert_eq!(
            candidate_kind(&candidates, &fundamental.id),
            Some(CandidateKind::Abandon)
        );
        assert_eq!(
            candidate_kind(&candidates, &hold.id),
            Some(CandidateKind::Hold)
        );
        assert!(candidates
            .iter()
            .find(|candidate| candidate.hypothesis_id == hold.id)
            .unwrap()
            .verdict
            .is_none());

        let decisions = store
            .select_branches(&RuleBasedSelector, &BranchPolicy::default())
            .unwrap();
        assert!(decisions.iter().any(|decision| matches!(
            decision.action,
            BranchAction::Abandon {
                recommend_status: crate::hypothesis::HypothesisStatus::Contradicted,
                ..
            } if decision.candidate.hypothesis_id == refuted.id
        )));
        assert!(decisions.iter().any(|decision| matches!(
            decision.action,
            BranchAction::Abandon {
                recommend_status: crate::hypothesis::HypothesisStatus::Superseded,
                ..
            } if decision.candidate.hypothesis_id == fundamental.id
        )));

        let _ = std::fs::remove_dir_all(path);
    }

    fn candidate_kind(candidates: &[BranchCandidate], id: &str) -> Option<CandidateKind> {
        candidates
            .iter()
            .find(|candidate| candidate.hypothesis_id == id)
            .map(|candidate| candidate.kind)
    }

    #[test]
    fn rule_based_selector_sorts_by_score_then_hypothesis_id() {
        let selector = RuleBasedSelector;
        let ranked = selector.rank(vec![
            candidate("hold", CandidateKind::Hold, None),
            candidate("z_spawn", CandidateKind::Spawn, Some(Confidence::High)),
            candidate("deepen", CandidateKind::Deepen, Some(Confidence::Medium)),
            candidate("spawn_low", CandidateKind::Spawn, Some(Confidence::Low)),
            candidate("abandon", CandidateKind::Abandon, Some(Confidence::High)),
            candidate("a_spawn", CandidateKind::Spawn, Some(Confidence::High)),
        ]);
        let ids = ranked
            .iter()
            .map(|candidate| candidate.hypothesis_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            ids,
            vec![
                "a_spawn",
                "z_spawn",
                "spawn_low",
                "deepen",
                "abandon",
                "hold"
            ]
        );
    }

    #[test]
    fn explore_policy_promotes_least_explored_candidate() {
        let path = temp_project_path("explore");
        let store = ProjectStore::init(&path, Some("Branch Demo")).unwrap();
        let exploited = record_with_verdict(
            &store,
            "High score but explored",
            Verdict::Affirmed,
            Confidence::High,
        );
        add_evidence(&store, &exploited.id, "first support");
        add_evidence(&store, &exploited.id, "second support");
        let first_unexplored = record_with_verdict(
            &store,
            "First unexplored",
            Verdict::Inconclusive(InconclusiveKind::Provisional {
                missing: vec!["new data".to_string()],
            }),
            Confidence::Low,
        );
        let second_unexplored = record_with_verdict(
            &store,
            "Second unexplored",
            Verdict::Inconclusive(InconclusiveKind::Provisional {
                missing: vec!["other data".to_string()],
            }),
            Confidence::High,
        );

        let decisions = store
            .select_branches(
                &RuleBasedSelector,
                &BranchPolicy {
                    explore_enabled: true,
                },
            )
            .unwrap();

        assert_eq!(decisions[0].candidate.hypothesis_id, first_unexplored.id);
        assert_eq!(decisions[0].selected_by, SelectionMode::Explore);
        assert!(decisions[1..]
            .iter()
            .all(|decision| decision.selected_by == SelectionMode::Exploit));
        assert!(decisions
            .iter()
            .any(|decision| decision.candidate.hypothesis_id == second_unexplored.id));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn propose_branch_patch_creates_deepen_and_spawn_add_step_proposals() {
        let path = temp_project_path("propose");
        let store = ProjectStore::init(&path, Some("Branch Demo")).unwrap();
        seed_flow(&store, "flow_branch");
        let deepen_hypothesis = record_with_verdict(
            &store,
            "Needs evidence",
            Verdict::Inconclusive(InconclusiveKind::Provisional {
                missing: vec!["observed support".to_string()],
            }),
            Confidence::Medium,
        );
        let spawn_hypothesis =
            record_with_verdict(&store, "Supported", Verdict::Affirmed, Confidence::High);
        let decisions = store
            .select_branches(&RuleBasedSelector, &BranchPolicy::default())
            .unwrap();
        let deepen = decisions
            .iter()
            .find(|decision| decision.candidate.hypothesis_id == deepen_hypothesis.id)
            .unwrap();
        let spawn = decisions
            .iter()
            .find(|decision| decision.candidate.hypothesis_id == spawn_hypothesis.id)
            .unwrap();

        let deepen_patch = store
            .propose_branch_patch("flow_branch", deepen, &proposed_step())
            .unwrap();
        assert_eq!(
            deepen_patch.title,
            format!("branch:deepen {}", deepen_hypothesis.id)
        );
        assert_eq!(
            deepen_patch.patch_json,
            r#"{"ops":[{"op":"add_step","id":"branch_scan","tool":"local/scan","needs":["scan"],"inputs":{"table":"artifact_1"},"params":{"gene":"TP53"},"outputs":{"report":"branch_report"}}]}"#
        );
        assert_eq!(deepen_patch.status, "pending");

        let spawn_patch = store
            .propose_branch_patch("flow_branch", spawn, &proposed_step())
            .unwrap();
        assert_eq!(
            spawn_patch.title,
            format!("branch:spawn {}", spawn_hypothesis.id)
        );
        assert!(spawn_patch.reason.contains("spawning a related branch"));

        let patches = store.list_graph_patches("flow_branch").unwrap();
        assert_eq!(patches.len(), 2);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn propose_branch_patch_rejects_abandon_and_hold_without_writing_patch() {
        let path = temp_project_path("reject");
        let store = ProjectStore::init(&path, Some("Branch Demo")).unwrap();
        seed_flow(&store, "flow_branch");
        let refuted = record_with_verdict(&store, "Refuted", Verdict::Refuted, Confidence::Medium);
        let hold = record_hypothesis(&store, "Waiting for verdict");
        let decisions = store
            .select_branches(&RuleBasedSelector, &BranchPolicy::default())
            .unwrap();

        for hypothesis_id in [refuted.id, hold.id] {
            let decision = decisions
                .iter()
                .find(|decision| decision.candidate.hypothesis_id == hypothesis_id)
                .unwrap();
            let error = store
                .propose_branch_patch("flow_branch", decision, &proposed_step())
                .unwrap_err();
            assert!(error.to_string().contains("abandon/hold"));
        }

        assert!(store.list_graph_patches("flow_branch").unwrap().is_empty());

        let _ = std::fs::remove_dir_all(path);
    }
}
