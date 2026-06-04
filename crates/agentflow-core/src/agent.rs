use std::collections::{BTreeMap, BTreeSet};

use rusqlite::params;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

use crate::argument::{ArgumentEngine, InconclusiveKind, RuleBasedEngine, Verdict, VerdictReport};
use crate::branch::{BranchAction, BranchDecision, BranchPolicy, ProposedStep, RuleBasedSelector};
use crate::handoff::{
    Cost, DecisionKind, DecisionPoint, DefaultPolicy, HandoffOption, InterventionPolicy, Risk,
    StepContext,
};
use crate::hypothesis::HypothesisStatus;
use crate::storage::{
    validate_param_value, ArtifactSummary, EventRecord, ProjectStore, StorageError, ToolParamSpec,
};
use crate::tool_select::{CapabilityQuery, Fit};

const AGENT_CYCLE_COMPLETED_EVENT: &str = "agent.cycle_completed";
const PARAMS_INFERRED_EVENT: &str = "agent.params_inferred";
const TOOL_GAP_HYPOTHESIS_MARKER: &str = "hypothesis_id = ";
const STANCE_ASSESSMENT_OBSERVATION_MARKER: &str = "observation_id = ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrichedProposal {
    pub decision: BranchDecision,
    pub matched_tool: Option<String>,
    pub matched_fit: Option<String>,
    pub match_reason: Option<String>,
    pub drafted_step: Option<ProposedStep>,
}

impl EnrichedProposal {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("enriched proposal serializes to JSON")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyConfig {
    pub apply: bool,
    pub auto_run: bool,
    pub flow: Option<String>,
    pub max_apply: u32,
    pub propose_synth: bool,
}

impl Default for ApplyConfig {
    fn default() -> Self {
        Self {
            apply: false,
            auto_run: false,
            flow: None,
            max_apply: 5,
            propose_synth: false,
        }
    }
}

pub trait ParamInferer {
    fn infer(&self, hypothesis_statement: &str, param_name: &str) -> Option<String>;
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NoopParamInferer;

impl ParamInferer for NoopParamInferer {
    fn infer(&self, _hypothesis_statement: &str, _param_name: &str) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppliedAction {
    LifecycleTransition {
        hypothesis_id: String,
        to: String,
    },
    GraphPatchApplied {
        flow_id: String,
        patch_id: String,
        step_id: String,
    },
    StepRun {
        step_id: String,
        observation_id: Option<String>,
    },
}

impl AppliedAction {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("applied action serializes to JSON")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyFailure {
    pub hypothesis_id: String,
    pub reason: String,
}

impl ApplyFailure {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("apply failure serializes to JSON")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CycleReport {
    pub checkpoint_id: String,
    pub provisional_verdicts: Vec<String>,
    pub strong_candidates: Vec<String>,
    pub raised_decisions: Vec<DecisionPoint>,
    pub branch_proposals: Vec<EnrichedProposal>,
    #[serde(default)]
    pub applied: Vec<AppliedAction>,
    #[serde(default)]
    pub apply_failures: Vec<ApplyFailure>,
    pub outcome: CycleOutcome,
}

impl CycleReport {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("cycle report serializes to JSON")
    }
}

impl Serialize for CycleReport {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let field_count = 7
            + usize::from(!self.applied.is_empty())
            + usize::from(!self.apply_failures.is_empty());
        let mut state = serializer.serialize_struct("CycleReport", field_count)?;
        state.serialize_field("schema_version", "agentflow.agent_cycle.v0")?;
        state.serialize_field("checkpoint_id", &self.checkpoint_id)?;
        state.serialize_field("provisional_verdicts", &self.provisional_verdicts)?;
        state.serialize_field("strong_candidates", &self.strong_candidates)?;
        state.serialize_field("raised_decisions", &self.raised_decisions)?;
        state.serialize_field("branch_proposals", &self.branch_proposals)?;
        if !self.applied.is_empty() {
            state.serialize_field("applied", &self.applied)?;
        }
        if !self.apply_failures.is_empty() {
            state.serialize_field("apply_failures", &self.apply_failures)?;
        }
        state.serialize_field("outcome", &self.outcome)?;
        state.end()
    }
}

impl ProjectStore {
    pub fn run_cycle(&self) -> Result<CycleReport, StorageError> {
        self.run_cycle_with_apply_config(ApplyConfig::default())
    }

    pub fn run_cycle_with_apply_config(
        &self,
        config: ApplyConfig,
    ) -> Result<CycleReport, StorageError> {
        self.run_cycle_with(config, &NoopParamInferer)
    }

    pub fn run_cycle_with(
        &self,
        config: ApplyConfig,
        inferer: &dyn ParamInferer,
    ) -> Result<CycleReport, StorageError> {
        let checkpoint = self.create_checkpoint("agent_cycle")?;
        let engine = RuleBasedEngine;
        let policy = DefaultPolicy;
        let mut provisional_verdicts = Vec::new();
        let mut strong_candidates = Vec::new();
        let mut raised_decisions = Vec::new();
        let mut branch_proposals = Vec::new();
        let mut applied = Vec::new();
        let mut apply_failures = Vec::new();

        for hypothesis in self.list_hypotheses()? {
            let evidence = self.evidence_for(&hypothesis.id)?;
            let preview = engine.render(&hypothesis.id, &evidence);
            match &preview.verdict {
                Verdict::Inconclusive(InconclusiveKind::Provisional { .. }) => {
                    self.render_verdict(&hypothesis.id, &engine, None)?;
                    provisional_verdicts.push(hypothesis.id.clone());
                    if config.apply && hypothesis.status == HypothesisStatus::Proposed {
                        let ctx = StepContext {
                            cost: Cost::Cheap,
                            reversible: true,
                            equivalent_branches: false,
                            conflicts_user_premise: false,
                            mutates_goal: false,
                            near_budget: applied_budget_count(&applied) as u32 >= config.max_apply,
                        };
                        if let Some(kind) = policy.assess(&ctx) {
                            let point = self.raise_decision_point(
                                kind,
                                &lifecycle_apply_digest(&hypothesis.id),
                                lifecycle_apply_options(&hypothesis.id),
                                0,
                            )?;
                            raised_decisions.push(point);
                        } else {
                            self.transition_hypothesis(
                                &hypothesis.id,
                                HypothesisStatus::UnderTest,
                                hypothesis.confidence,
                            )?;
                            applied.push(AppliedAction::LifecycleTransition {
                                hypothesis_id: hypothesis.id,
                                to: HypothesisStatus::UnderTest.as_str().to_string(),
                            });
                        }
                    }
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
        let artifacts = self.list_artifacts()?;
        let available_input_types = available_input_types(&artifacts);
        let available = available_artifacts(&artifacts);
        let mut pending_tool_gap_hypotheses = if config.propose_synth {
            self.pending_tool_gap_hypothesis_ids()?
        } else {
            BTreeSet::new()
        };
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
                    let (proposal, inferred_param_names) = self.enrich_branch_proposal(
                        decision,
                        &available_input_types,
                        &available,
                        inferer,
                    )?;
                    if config.propose_synth && proposal.matched_tool.is_none() {
                        let hypothesis_id = &proposal.decision.candidate.hypothesis_id;
                        if pending_tool_gap_hypotheses.insert(hypothesis_id.clone()) {
                            let point = self.raise_decision_point(
                                DecisionKind::ToolGap,
                                &tool_gap_digest(&proposal.decision),
                                tool_gap_options(hypothesis_id),
                                0,
                            )?;
                            raised_decisions.push(point);
                        }
                    }
                    if config.apply {
                        if let (Some(flow_id), Some(step)) =
                            (config.flow.as_deref(), proposal.drafted_step.as_ref())
                        {
                            let ctx = StepContext {
                                cost: Cost::Moderate,
                                reversible: true,
                                equivalent_branches: self.has_equivalent_tool_branches(
                                    &proposal.decision,
                                    &available_input_types,
                                )?,
                                conflicts_user_premise: false,
                                mutates_goal: false,
                                near_budget: applied_budget_count(&applied) as u32
                                    >= config.max_apply,
                            };
                            if let Some(kind) = policy.assess(&ctx) {
                                let point = self.raise_decision_point(
                                    kind,
                                    &graph_patch_apply_digest(&proposal.decision, flow_id),
                                    graph_patch_apply_options(
                                        &proposal.decision.candidate.hypothesis_id,
                                        flow_id,
                                    ),
                                    0,
                                )?;
                                raised_decisions.push(point);
                            } else {
                                match self.apply_branch_patch_for_proposal(
                                    flow_id,
                                    &proposal.decision,
                                    step,
                                ) {
                                    Ok(actions) => {
                                        for action in actions {
                                            if let AppliedAction::GraphPatchApplied {
                                                flow_id,
                                                step_id,
                                                ..
                                            } = &action
                                            {
                                                self.emit_inferred_params_for_step(
                                                    flow_id,
                                                    step_id,
                                                    &proposal.decision.candidate.hypothesis_id,
                                                    step,
                                                    &inferred_param_names,
                                                )?;
                                            }
                                            let auto_run_target = match &action {
                                                AppliedAction::GraphPatchApplied {
                                                    flow_id,
                                                    step_id,
                                                    ..
                                                } if config.auto_run => {
                                                    Some((flow_id.clone(), step_id.clone()))
                                                }
                                                _ => None,
                                            };
                                            applied.push(action);
                                            if let Some((flow_id, step_id)) = auto_run_target {
                                                self.auto_run_applied_step(
                                                    &proposal.decision.candidate.hypothesis_id,
                                                    &flow_id,
                                                    &step_id,
                                                    &mut applied,
                                                    &mut apply_failures,
                                                    &mut raised_decisions,
                                                )?;
                                            }
                                        }
                                    }
                                    Err(error) => apply_failures.push(ApplyFailure {
                                        hypothesis_id: proposal
                                            .decision
                                            .candidate
                                            .hypothesis_id
                                            .clone(),
                                        reason: error.to_string(),
                                    }),
                                }
                            }
                        }
                    }
                    branch_proposals.push(proposal);
                }
                BranchAction::Hold { .. } => {}
            }
        }

        let outcome = if raised_decisions.is_empty() {
            if provisional_verdicts.is_empty() && branch_proposals.is_empty() && applied.is_empty()
            {
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
            applied,
            apply_failures,
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

impl ProjectStore {
    fn pending_tool_gap_hypothesis_ids(&self) -> Result<BTreeSet<String>, StorageError> {
        Ok(self
            .pending_decision_points()?
            .into_iter()
            .filter(|point| point.kind == DecisionKind::ToolGap)
            .filter_map(|point| tool_gap_hypothesis_id(&point.digest))
            .collect())
    }

    fn pending_stance_assessment_observation_ids(&self) -> Result<BTreeSet<String>, StorageError> {
        Ok(self
            .pending_decision_points()?
            .into_iter()
            .filter(|point| point.kind == DecisionKind::StanceAssessment)
            .filter_map(|point| stance_assessment_observation_id(&point.digest))
            .collect())
    }

    fn enrich_branch_proposal(
        &self,
        decision: BranchDecision,
        available_input_types: &[String],
        available: &[(String, String)],
        inferer: &dyn ParamInferer,
    ) -> Result<(EnrichedProposal, Vec<String>), StorageError> {
        let query = CapabilityQuery {
            desired_output_type: None,
            available_input_types: available_input_types.to_vec(),
            keywords: proposal_keywords(&decision.candidate.statement),
        };
        let top = self.match_tools(&query)?.into_iter().next();

        let Some(candidate) = top else {
            return Ok((
                EnrichedProposal {
                    decision,
                    matched_tool: None,
                    matched_fit: None,
                    match_reason: None,
                    drafted_step: None,
                },
                Vec::new(),
            ));
        };

        let executable = self.executable_tool(&candidate.tool_ref)?;
        let mut drafted_step = self.draft_step_for(&candidate.tool_ref, available)?;
        let inferred_param_names = infer_replace_params(
            &mut drafted_step,
            &decision.candidate.statement,
            inferer,
            &executable.params,
        );
        let needs = self.infer_step_needs(&drafted_step)?;
        let drafted_step = ProposedStep {
            needs,
            ..drafted_step
        };
        Ok((
            EnrichedProposal {
                decision,
                matched_tool: Some(candidate.tool_ref),
                matched_fit: Some(candidate.fit.as_str().to_string()),
                match_reason: Some(candidate.reason),
                drafted_step: Some(drafted_step),
            },
            inferred_param_names,
        ))
    }

    fn has_equivalent_tool_branches(
        &self,
        decision: &BranchDecision,
        available_input_types: &[String],
    ) -> Result<bool, StorageError> {
        let query = CapabilityQuery {
            desired_output_type: None,
            available_input_types: available_input_types.to_vec(),
            keywords: proposal_keywords(&decision.candidate.statement),
        };
        let candidate_count = self
            .match_tools(&query)?
            .into_iter()
            .filter(|candidate| matches!(candidate.fit, Fit::High | Fit::Medium))
            .take(2)
            .count();
        Ok(candidate_count > 1)
    }

    fn apply_branch_patch_for_proposal(
        &self,
        flow_id: &str,
        decision: &BranchDecision,
        step: &ProposedStep,
    ) -> Result<Vec<AppliedAction>, StorageError> {
        let patch = self.propose_branch_patch(flow_id, decision, step)?;
        self.approve_graph_patch(&patch.id)?;
        let application = self.apply_graph_patch(&patch.id)?;
        Ok(application
            .applied_steps
            .into_iter()
            .map(|step_id| AppliedAction::GraphPatchApplied {
                flow_id: flow_id.to_string(),
                patch_id: patch.id.clone(),
                step_id,
            })
            .collect())
    }

    fn emit_inferred_params_for_step(
        &self,
        flow_id: &str,
        step_id: &str,
        hypothesis_id: &str,
        step: &ProposedStep,
        inferred_param_names: &[String],
    ) -> Result<(), StorageError> {
        let params = inferred_param_payloads(step, inferred_param_names);
        if params.is_empty() {
            return Ok(());
        }

        self.append_event(EventRecord {
            flow_id: Some(flow_id.to_string()),
            step_id: Some(step_id.to_string()),
            run_id: None,
            event_type: PARAMS_INFERRED_EVENT.to_string(),
            payload_json: params_inferred_payload_json(flow_id, step_id, hypothesis_id, params),
        })?;
        self.touch_project()?;
        Ok(())
    }

    fn auto_run_applied_step(
        &self,
        hypothesis_id: &str,
        flow_id: &str,
        step_id: &str,
        applied: &mut Vec<AppliedAction>,
        apply_failures: &mut Vec<ApplyFailure>,
        raised_decisions: &mut Vec<DecisionPoint>,
    ) -> Result<(), StorageError> {
        let step_ref = format!("step:{flow_id}/{step_id}");
        let summary = match self.run_step_ref(&step_ref) {
            Ok(summary) => summary,
            Err(error) => {
                apply_failures.push(ApplyFailure {
                    hypothesis_id: hypothesis_id.to_string(),
                    reason: auto_run_failure_reason(step_id, &error.to_string()),
                });
                applied.push(AppliedAction::StepRun {
                    step_id: step_id.to_string(),
                    observation_id: None,
                });
                return Ok(());
            }
        };

        let source_step_id = summary
            .attempts
            .first()
            .map(|attempt| attempt.step_id.as_str())
            .unwrap_or(step_ref.as_str());
        let observation_id = self.latest_observation_id_for_step(source_step_id)?;
        if summary.failed_steps > 0 {
            let status = summary
                .attempts
                .first()
                .map(|attempt| attempt.status.as_str())
                .unwrap_or("unknown");
            apply_failures.push(ApplyFailure {
                hypothesis_id: hypothesis_id.to_string(),
                reason: auto_run_failure_reason(
                    step_id,
                    &format!("completed with status {status}"),
                ),
            });
        }

        if let Some(observation_id) = observation_id.as_deref() {
            self.raise_stance_assessment_for_observation(
                hypothesis_id,
                step_id,
                observation_id,
                raised_decisions,
            )?;
        }

        applied.push(AppliedAction::StepRun {
            step_id: step_id.to_string(),
            observation_id,
        });
        Ok(())
    }

    fn raise_stance_assessment_for_observation(
        &self,
        hypothesis_id: &str,
        step_id: &str,
        observation_id: &str,
        raised_decisions: &mut Vec<DecisionPoint>,
    ) -> Result<(), StorageError> {
        let pending_observations = self.pending_stance_assessment_observation_ids()?;
        if pending_observations.contains(observation_id) {
            return Ok(());
        }

        let observation = self.inspect_observation(observation_id)?;
        let hypothesis = self.inspect_hypothesis(hypothesis_id)?;
        let inferred_params = inferred_params_for_observation(
            self,
            observation.flow_id.as_deref(),
            observation.step_id.as_deref(),
        )?;
        let mut digest = stance_assessment_digest(
            step_id,
            observation_id,
            &observation.summary,
            hypothesis_id,
            &hypothesis.statement,
        );
        if !inferred_params.is_empty() {
            digest.push('\n');
            digest.push_str(&inferred_param_warning(&inferred_params));
        }
        let point = self.raise_decision_point(
            DecisionKind::StanceAssessment,
            &digest,
            stance_assessment_options(hypothesis_id, observation_id),
            2,
        )?;
        raised_decisions.push(point);
        Ok(())
    }

    fn latest_observation_id_for_step(
        &self,
        source_step_id: &str,
    ) -> Result<Option<String>, StorageError> {
        Ok(self
            .list_observations()?
            .into_iter()
            .rev()
            .find(|observation| observation.step_id.as_deref() == Some(source_step_id))
            .map(|observation| observation.id))
    }

    pub fn inferred_params_for_step(
        &self,
        flow_id: &str,
        step_id: &str,
    ) -> Result<Vec<(String, String)>, StorageError> {
        let flow_id = flow_id.trim();
        let step_id = step_id.trim();
        if flow_id.is_empty() {
            return Err(StorageError::InvalidInput(
                "flow id must not be empty".to_string(),
            ));
        }
        if step_id.is_empty() {
            return Err(StorageError::InvalidInput(
                "step id must not be empty".to_string(),
            ));
        }

        let mut stmt = self.connection().prepare(
            "SELECT id, flow_id, step_id, payload_json
             FROM events
             WHERE event_type = ?1
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![PARAMS_INFERRED_EVENT], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;

        let mut params = Vec::new();
        for row in rows {
            let (event_id, event_flow_id, event_step_id, payload_json) = row?;
            let payload = params_inferred_payload_from_json(&event_id, &payload_json)?;
            let candidate_flow_id = event_flow_id.unwrap_or(payload.flow_id);
            let candidate_step_id = event_step_id.unwrap_or(payload.step_id);
            if candidate_flow_id == flow_id && step_ids_match(flow_id, step_id, &candidate_step_id)
            {
                for param in payload.params {
                    let name = param.name.trim();
                    if name.is_empty() {
                        continue;
                    }
                    params.push((name.to_string(), param.value.trim().to_string()));
                }
            }
        }

        Ok(params)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ParamsInferredPayload {
    flow_id: String,
    step_id: String,
    hypothesis_id: String,
    params: Vec<InferredParamPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct InferredParamPayload {
    name: String,
    value: String,
}

fn inferred_param_payloads(
    step: &ProposedStep,
    inferred_param_names: &[String],
) -> Vec<InferredParamPayload> {
    inferred_param_names
        .iter()
        .filter_map(|name| {
            step.params
                .iter()
                .find(|(param_name, _)| param_name == name)
                .map(|(_, value)| InferredParamPayload {
                    name: name.clone(),
                    value: value.clone(),
                })
        })
        .collect()
}

fn params_inferred_payload_json(
    flow_id: &str,
    step_id: &str,
    hypothesis_id: &str,
    params: Vec<InferredParamPayload>,
) -> String {
    serde_json::to_string(&ParamsInferredPayload {
        flow_id: flow_id.to_string(),
        step_id: step_id.to_string(),
        hypothesis_id: hypothesis_id.to_string(),
        params,
    })
    .expect("params inferred payload serializes to JSON")
}

fn params_inferred_payload_from_json(
    event_id: &str,
    payload_json: &str,
) -> Result<ParamsInferredPayload, StorageError> {
    serde_json::from_str(payload_json).map_err(|err| {
        StorageError::InvalidInput(format!(
            "params inferred event {event_id} has invalid payload: {err}"
        ))
    })
}

fn inferred_params_for_observation(
    store: &ProjectStore,
    flow_id: Option<&str>,
    step_id: Option<&str>,
) -> Result<Vec<(String, String)>, StorageError> {
    let (Some(flow_id), Some(step_id)) = (flow_id, step_id) else {
        return Ok(Vec::new());
    };
    store.inferred_params_for_step(flow_id, step_id)
}

fn inferred_param_warning(params: &[(String, String)]) -> String {
    let params = params
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("⚠ 该结果依赖 LLM 推断的未确认参数：{params}（请人工确认参数正确再据此判定立场）")
}

fn step_ids_match(flow_id: &str, requested: &str, candidate: &str) -> bool {
    step_id_variants(flow_id, requested)
        .intersection(&step_id_variants(flow_id, candidate))
        .next()
        .is_some()
}

fn step_id_variants(flow_id: &str, step_id: &str) -> BTreeSet<String> {
    let step_id = step_id.trim();
    let mut variants = BTreeSet::from([step_id.to_string()]);
    if let Some(local_id) = canonical_step_local_id(flow_id, step_id) {
        variants.insert(local_id.to_string());
    } else {
        variants.insert(format!("step:{flow_id}/{step_id}"));
    }
    variants
}

fn canonical_step_local_id<'a>(flow_id: &str, step_id: &'a str) -> Option<&'a str> {
    let rest = step_id.strip_prefix("step:")?;
    let (step_flow_id, local_id) = rest.split_once('/')?;
    if step_flow_id == flow_id && !local_id.trim().is_empty() {
        Some(local_id)
    } else {
        None
    }
}

fn applied_budget_count(applied: &[AppliedAction]) -> usize {
    applied
        .iter()
        .filter(|action| {
            matches!(
                action,
                AppliedAction::LifecycleTransition { .. } | AppliedAction::GraphPatchApplied { .. }
            )
        })
        .count()
}

fn auto_run_failure_reason(step_id: &str, reason: &str) -> String {
    format!("auto-run step {step_id} failed: {reason}")
}

fn infer_replace_params(
    step: &mut ProposedStep,
    hypothesis_statement: &str,
    inferer: &dyn ParamInferer,
    param_specs: &BTreeMap<String, ToolParamSpec>,
) -> Vec<String> {
    let mut inferred_param_names = Vec::new();
    for (param_name, param_value) in &mut step.params {
        let placeholder = format!("REPLACE_{param_name}");
        if param_value != &placeholder {
            continue;
        }

        let Some(inferred) = inferer.infer(hypothesis_statement, param_name) else {
            continue;
        };
        let trimmed = inferred.trim();
        if !trimmed.is_empty() {
            if let Some(spec) = param_specs.get(param_name) {
                if validate_param_value(spec, trimmed).is_err() {
                    continue;
                }
            }
            *param_value = trimmed.to_string();
            inferred_param_names.push(param_name.clone());
        }
    }
    inferred_param_names
}

fn proposal_keywords(statement: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut keywords = Vec::new();
    for token in statement
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|token| token.len() >= 4)
    {
        if seen.insert(token.clone()) {
            keywords.push(token);
            if keywords.len() == 8 {
                break;
            }
        }
    }
    keywords
}

fn available_input_types(artifacts: &[ArtifactSummary]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut types = Vec::new();
    for artifact in artifacts {
        if seen.insert(artifact.artifact_type.clone()) {
            types.push(artifact.artifact_type.clone());
        }
    }
    types
}

fn available_artifacts(artifacts: &[ArtifactSummary]) -> Vec<(String, String)> {
    artifacts
        .iter()
        .map(|artifact| (artifact.artifact_type.clone(), artifact.id.clone()))
        .collect()
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

fn lifecycle_apply_digest(hypothesis_id: &str) -> String {
    format!(
        "假设 {hypothesis_id} 已产生 provisional 判决，自动推进到 under_test 前触发刹车，需要人类确认"
    )
}

fn lifecycle_apply_options(hypothesis_id: &str) -> Vec<HandoffOption> {
    vec![
        HandoffOption {
            label: "推进".to_string(),
            direction: format!("将假设 {hypothesis_id} 推进到 under_test"),
            cost: Cost::Cheap,
            risk: Risk::Low,
            reversible: true,
        },
        HandoffOption {
            label: "暂停".to_string(),
            direction: format!("保持假设 {hypothesis_id} 当前状态并等待人工处理"),
            cost: Cost::Cheap,
            risk: Risk::Low,
            reversible: true,
        },
    ]
}

fn graph_patch_apply_digest(decision: &BranchDecision, flow_id: &str) -> String {
    format!(
        "分支 {} 已产生可落地步骤，自动应用到 flow {flow_id} 前触发刹车，需要人类确认",
        decision.candidate.hypothesis_id
    )
}

fn graph_patch_apply_options(hypothesis_id: &str, flow_id: &str) -> Vec<HandoffOption> {
    vec![
        HandoffOption {
            label: "应用补丁".to_string(),
            direction: format!("将分支 {hypothesis_id} 的新步骤应用到 flow {flow_id}"),
            cost: Cost::Moderate,
            risk: Risk::Low,
            reversible: true,
        },
        HandoffOption {
            label: "仅保留提议".to_string(),
            direction: format!("不修改 flow {flow_id}，仅保留分支 {hypothesis_id} 的提议"),
            cost: Cost::Cheap,
            risk: Risk::Low,
            reversible: true,
        },
    ]
}

fn stance_assessment_digest(
    step_id: &str,
    observation_id: &str,
    observation_summary: &str,
    hypothesis_id: &str,
    hypothesis_statement: &str,
) -> String {
    format!(
        "分析步骤 {step_id} 产出真实发现：{observation_summary}。{STANCE_ASSESSMENT_OBSERVATION_MARKER}{observation_id}；请判定它对假设「{hypothesis_statement}」的立场。若支持/反对，运行：evidence link --hypothesis {hypothesis_id} --observation {observation_id} --stance supports|contradicts --grade observed"
    )
}

fn stance_assessment_options(hypothesis_id: &str, observation_id: &str) -> Vec<HandoffOption> {
    vec![
        HandoffOption {
            label: "supports — 该发现支持假设".to_string(),
            direction: format!("将观察 {observation_id} 作为支持证据链接到假设 {hypothesis_id}"),
            cost: Cost::Cheap,
            risk: Risk::Low,
            reversible: true,
        },
        HandoffOption {
            label: "contradicts — 反对假设".to_string(),
            direction: format!("将观察 {observation_id} 作为反对证据链接到假设 {hypothesis_id}"),
            cost: Cost::Cheap,
            risk: Risk::Low,
            reversible: true,
        },
        HandoffOption {
            label: "inconclusive — 暂无法判定/需更多证据".to_string(),
            direction: format!("暂不将观察 {observation_id} 链接为假设 {hypothesis_id} 的立场证据"),
            cost: Cost::Cheap,
            risk: Risk::Low,
            reversible: true,
        },
    ]
}

fn stance_assessment_observation_id(digest: &str) -> Option<String> {
    let rest = digest.split_once(STANCE_ASSESSMENT_OBSERVATION_MARKER)?.1;
    rest.split('；').next().map(str::trim).and_then(|id| {
        if id.is_empty() {
            None
        } else {
            Some(id.to_string())
        }
    })
}

fn tool_gap_digest(decision: &BranchDecision) -> String {
    let hypothesis_id = &decision.candidate.hypothesis_id;
    let statement = &decision.candidate.statement;
    let synth_name = tool_gap_synth_name(statement);
    let description = statement.replace('"', "\\\"");
    format!(
        "§15 决策痕迹：{TOOL_GAP_HYPOTHESIS_MARKER}{hypothesis_id}；能力需求 = {statement}；现状 = 无注册工具匹配（registry_match 失败）；建议 = 合成一个 exploratory 工具：`agentflow synth --name {synth_name} --description \"{description}\" --fixture <你的已知答案文件> --expect <...>`；需人类批准 + 提供验证 fixture"
    )
}

fn tool_gap_options(hypothesis_id: &str) -> Vec<HandoffOption> {
    vec![
        HandoffOption {
            label: "合成一个工具（提供 fixture 后 synth）".to_string(),
            direction: format!(
                "为假设 {hypothesis_id} 准备验证 fixture，并在批准后运行 agentflow synth"
            ),
            cost: Cost::Moderate,
            risk: Risk::Medium,
            reversible: true,
        },
        HandoffOption {
            label: "注册一个已有工具".to_string(),
            direction: format!("为假设 {hypothesis_id} 查找并注册已有工具"),
            cost: Cost::Cheap,
            risk: Risk::Low,
            reversible: true,
        },
        HandoffOption {
            label: "跳过该分支".to_string(),
            direction: format!("保持假设 {hypothesis_id} 不推进该能力缺口分支"),
            cost: Cost::Cheap,
            risk: Risk::Low,
            reversible: true,
        },
    ]
}

fn tool_gap_hypothesis_id(digest: &str) -> Option<String> {
    let rest = digest.split_once(TOOL_GAP_HYPOTHESIS_MARKER)?.1;
    rest.split('；').next().map(str::trim).and_then(|id| {
        if id.is_empty() {
            None
        } else {
            Some(id.to_string())
        }
    })
}

fn tool_gap_synth_name(statement: &str) -> String {
    let mut name = String::from("tool_gap");
    for token in statement
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|token| token.len() >= 3)
        .take(4)
    {
        name.push('_');
        name.push_str(&token);
    }
    name
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
    serde_json::to_string(&CycleCompletedPayload {
        checkpoint_id: report.checkpoint_id.clone(),
        provisional_verdict_count: report.provisional_verdicts.len(),
        strong_candidate_count: report.strong_candidates.len(),
        raised_decision_count: report.raised_decisions.len(),
        branch_proposal_count: report.branch_proposals.len(),
        outcome: report.outcome,
    })
    .expect("cycle completed payload serializes to JSON")
}

#[derive(Debug, Serialize, Deserialize)]
struct CycleCompletedPayload {
    checkpoint_id: String,
    provisional_verdict_count: usize,
    strong_candidate_count: usize,
    raised_decision_count: usize,
    branch_proposal_count: usize,
    outcome: CycleOutcome,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rusqlite::params;

    use crate::argument::{EvidenceGrade, EvidenceLinkRequest, Stance, VerdictTag};
    use crate::branch::{
        BranchAction, BranchCandidate, BranchDecision, CandidateKind, ProposedStep, SelectionMode,
    };
    use crate::handoff::DecisionKind;
    use crate::hypothesis::{Confidence, HypothesisRequest, HypothesisStatus};
    use crate::storage::{
        now_unix_seconds, ArtifactImportMode, ArtifactImportRequest, ComputedArtifactRequest,
        FlowDraft, ProjectStore, ToolSpec,
    };

    use super::{
        proposal_keywords, AppliedAction, ApplyConfig, CycleOutcome, NoopParamInferer, ParamInferer,
    };

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

    fn sample_decision() -> BranchDecision {
        BranchDecision {
            candidate: BranchCandidate {
                hypothesis_id: "hypothesis_1".to_string(),
                statement: "Marker evidence needs deeper validation".to_string(),
                verdict: Some(VerdictTag::InconclusiveProvisional),
                confidence: Some(Confidence::Medium),
                kind: CandidateKind::Deepen,
                evidence_count: 2,
                score: 33,
            },
            action: BranchAction::Deepen {
                reason: "Need \"more\"\n".to_string(),
            },
            selected_by: SelectionMode::Explore,
        }
    }

    fn sample_proposal() -> super::EnrichedProposal {
        super::EnrichedProposal {
            decision: sample_decision(),
            matched_tool: Some("analysis/marker".to_string()),
            matched_fit: Some("medium".to_string()),
            match_reason: Some("input \"ok\"\n".to_string()),
            drafted_step: Some(ProposedStep {
                id: "step_marker".to_string(),
                tool: "analysis/marker".to_string(),
                needs: vec!["producer".to_string()],
                inputs: vec![("expression_table".to_string(), "artifact_1".to_string())],
                params: vec![("gene".to_string(), "TP53".to_string())],
                outputs: vec![("report".to_string(), "marker_report".to_string())],
            }),
        }
    }

    #[test]
    fn json_outputs_match_legacy_bytes() {
        let proposal = sample_proposal();
        assert_eq!(
            proposal.to_json(),
            "{\"decision\":{\"candidate\":{\"hypothesis_id\":\"hypothesis_1\",\"statement\":\"Marker evidence needs deeper validation\",\"verdict\":\"inconclusive_provisional\",\"confidence\":\"medium\",\"kind\":\"deepen\",\"evidence_count\":2,\"score\":33},\"action\":{\"kind\":\"deepen\",\"reason\":\"Need \\\"more\\\"\\n\"},\"selected_by\":\"explore\"},\"matched_tool\":\"analysis/marker\",\"matched_fit\":\"medium\",\"match_reason\":\"input \\\"ok\\\"\\n\",\"drafted_step\":{\"id\":\"step_marker\",\"tool\":\"analysis/marker\",\"needs\":[\"producer\"],\"inputs\":{\"expression_table\":\"artifact_1\"},\"params\":{\"gene\":\"TP53\"},\"outputs\":{\"report\":\"marker_report\"}}}"
        );

        assert_eq!(
            (AppliedAction::LifecycleTransition {
                hypothesis_id: "hypothesis_1".to_string(),
                to: "under_test".to_string(),
            })
            .to_json(),
            "{\"type\":\"lifecycle_transition\",\"hypothesis_id\":\"hypothesis_1\",\"to\":\"under_test\"}"
        );
        assert_eq!(
            (AppliedAction::GraphPatchApplied {
                flow_id: "flow_1".to_string(),
                patch_id: "patch_1".to_string(),
                step_id: "step_1".to_string(),
            })
            .to_json(),
            "{\"type\":\"graph_patch_applied\",\"flow_id\":\"flow_1\",\"patch_id\":\"patch_1\",\"step_id\":\"step_1\"}"
        );
        assert_eq!(
            (AppliedAction::StepRun {
                step_id: "step_1".to_string(),
                observation_id: Some("observation_1".to_string()),
            })
            .to_json(),
            "{\"type\":\"step_run\",\"step_id\":\"step_1\",\"observation_id\":\"observation_1\"}"
        );
        let step_run_without_observation = AppliedAction::StepRun {
            step_id: "step_1".to_string(),
            observation_id: None,
        };
        assert_eq!(
            step_run_without_observation.to_json(),
            "{\"type\":\"step_run\",\"step_id\":\"step_1\",\"observation_id\":null}"
        );

        let failure = super::ApplyFailure {
            hypothesis_id: "hypothesis_1".to_string(),
            reason: "Quote \" and newline\n".to_string(),
        };
        assert_eq!(
            failure.to_json(),
            "{\"hypothesis_id\":\"hypothesis_1\",\"reason\":\"Quote \\\" and newline\\n\"}"
        );

        let report = super::CycleReport {
            checkpoint_id: "checkpoint_1".to_string(),
            provisional_verdicts: vec!["hypothesis_1".to_string()],
            strong_candidates: vec!["hypothesis_2".to_string()],
            raised_decisions: Vec::new(),
            branch_proposals: vec![proposal.clone()],
            applied: vec![step_run_without_observation.clone()],
            apply_failures: vec![failure.clone()],
            outcome: CycleOutcome::Advanced,
        };
        assert_eq!(
            report.to_json(),
            format!(
                "{{\"schema_version\":\"agentflow.agent_cycle.v0\",\"checkpoint_id\":\"checkpoint_1\",\"provisional_verdicts\":[\"hypothesis_1\"],\"strong_candidates\":[\"hypothesis_2\"],\"raised_decisions\":[],\"branch_proposals\":[{}],\"applied\":[{}],\"apply_failures\":[{}],\"outcome\":\"advanced\"}}",
                proposal.to_json(),
                step_run_without_observation.to_json(),
                failure.to_json()
            )
        );

        let empty_report = super::CycleReport {
            checkpoint_id: "checkpoint_empty".to_string(),
            provisional_verdicts: Vec::new(),
            strong_candidates: Vec::new(),
            raised_decisions: Vec::new(),
            branch_proposals: Vec::new(),
            applied: Vec::new(),
            apply_failures: Vec::new(),
            outcome: CycleOutcome::Idle,
        };
        assert_eq!(
            empty_report.to_json(),
            "{\"schema_version\":\"agentflow.agent_cycle.v0\",\"checkpoint_id\":\"checkpoint_empty\",\"provisional_verdicts\":[],\"strong_candidates\":[],\"raised_decisions\":[],\"branch_proposals\":[],\"outcome\":\"idle\"}"
        );
        assert_eq!(
            super::cycle_completed_payload_json(&report),
            "{\"checkpoint_id\":\"checkpoint_1\",\"provisional_verdict_count\":1,\"strong_candidate_count\":1,\"raised_decision_count\":0,\"branch_proposal_count\":1,\"outcome\":\"advanced\"}"
        );
    }

    #[test]
    fn legacy_handwritten_payloads_parse_with_json_whitespace_and_ordering() {
        let report: super::CycleReport = serde_json::from_str(
            r#"{
                "outcome": "handed_off",
                "branch_proposals": [
                    {
                        "drafted_step": {
                            "outputs": {"report": "marker_report"},
                            "params": {"gene": "TP53"},
                            "inputs": {"expression_table": "artifact_1"},
                            "needs": ["producer"],
                            "tool": "analysis/marker",
                            "id": "step_marker"
                        },
                        "match_reason": "legacy match",
                        "matched_fit": "medium",
                        "matched_tool": "analysis/marker",
                        "decision": {
                            "selected_by": "explore",
                            "action": {
                                "reason": "legacy deepen",
                                "kind": "deepen"
                            },
                            "candidate": {
                                "score": 33,
                                "evidence_count": 2,
                                "kind": "deepen",
                                "confidence": "medium",
                                "verdict": "inconclusive_provisional",
                                "statement": "Legacy candidate",
                                "hypothesis_id": "hypothesis_legacy"
                            }
                        }
                    }
                ],
                "raised_decisions": [],
                "strong_candidates": ["hypothesis_strong"],
                "provisional_verdicts": ["hypothesis_legacy"],
                "checkpoint_id": "checkpoint_legacy",
                "schema_version": "agentflow.agent_cycle.v0"
            }"#,
        )
        .unwrap();

        assert_eq!(report.checkpoint_id, "checkpoint_legacy");
        assert_eq!(report.provisional_verdicts, vec!["hypothesis_legacy"]);
        assert_eq!(report.strong_candidates, vec!["hypothesis_strong"]);
        assert_eq!(report.branch_proposals.len(), 1);
        assert!(report.applied.is_empty());
        assert!(report.apply_failures.is_empty());
        assert_eq!(report.outcome, CycleOutcome::HandedOff);
        let step = report.branch_proposals[0].drafted_step.as_ref().unwrap();
        assert_eq!(
            step.inputs,
            vec![("expression_table".to_string(), "artifact_1".to_string())]
        );

        let action: AppliedAction = serde_json::from_str(
            r#"{
                "observation_id": null,
                "step_id": "step_legacy",
                "type": "step_run"
            }"#,
        )
        .unwrap();
        assert_eq!(
            action,
            AppliedAction::StepRun {
                step_id: "step_legacy".to_string(),
                observation_id: None,
            }
        );
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

    fn register_marker_tool(store: &ProjectStore) {
        let spec = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
namespace: analysis
name: marker_deepen
version: 0.1.0
maturity: verified
description: Marker evidence deepening report
inputs:
  expression_table:
    type: ExpressionTable
    required: true
params:
  threshold:
    type: string
    required: false
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#,
        )
        .unwrap();
        store.register_tool(spec).unwrap();
    }

    fn register_gene_marker_tool(store: &ProjectStore) {
        let spec = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
namespace: analysis
name: gene_marker_deepen
version: 0.1.0
maturity: verified
description: Marker gene deepening report for pathway validation
inputs:
  expression_table:
    type: ExpressionTable
    required: true
params:
  gene:
    type: string
    required: true
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#,
        )
        .unwrap();
        store.register_tool(spec).unwrap();
    }

    fn write_auto_run_marker_script(root: &std::path::Path, fail: bool) -> PathBuf {
        let script_path = root.join(if fail {
            "auto_run_marker_fail.sh"
        } else {
            "auto_run_marker.sh"
        });
        let body = if fail {
            "echo 'fixture auto-run failure' >&2\nexit 7\n".to_string()
        } else {
            r#"cat "$AGENTFLOW_INPUT_EXPRESSION_TABLE" >/dev/null
printf '# Marker report\nGene: THRSP\nscore: 0.61\n' > "$AGENTFLOW_OUTPUT_MARKER_REPORT"
"#
            .to_string()
        };
        std::fs::write(&script_path, body).unwrap();
        script_path
    }

    fn register_exploratory_marker_tool(store: &ProjectStore, script_path: &std::path::Path) {
        let command = script_path.display();
        let spec = ToolSpec::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.tool.v0
namespace: analysis
name: marker_deepen
version: 0.1.0
maturity: exploratory
description: Marker evidence deepening report for pathway validation
inputs:
  expression_table:
    type: ExpressionTable
    required: true
outputs:
  marker_report:
    type: Markdown
    observer: marker_report
runtime:
  backend: local
  command:
    - /bin/sh
    - {command}
"#
        ))
        .unwrap();
        store.register_tool(spec).unwrap();
    }

    fn register_constrained_exploratory_marker_tool(
        store: &ProjectStore,
        script_path: &std::path::Path,
    ) {
        let command = script_path.display();
        let spec = ToolSpec::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.tool.v0
namespace: analysis
name: marker_deepen
version: 0.1.0
maturity: exploratory
description: Marker gene deepening report for pathway validation
inputs:
  expression_table:
    type: ExpressionTable
    required: true
params:
  gene:
    type: string
    required: true
    pattern: "^[A-Z0-9-]+$"
outputs:
  marker_report:
    type: Markdown
    observer: marker_report
runtime:
  backend: local
  command:
    - /bin/sh
    - {command}
"#
        ))
        .unwrap();
        store.register_tool(spec).unwrap();
    }

    struct StubParamInferer;

    impl ParamInferer for StubParamInferer {
        fn infer(&self, hypothesis_statement: &str, param_name: &str) -> Option<String> {
            assert!(hypothesis_statement.contains("THRSP"));
            assert_eq!(param_name, "gene");
            Some("THRSP".to_string())
        }
    }

    struct InvalidParamInferer;

    impl ParamInferer for InvalidParamInferer {
        fn infer(&self, hypothesis_statement: &str, param_name: &str) -> Option<String> {
            assert!(hypothesis_statement.contains("THRSP"));
            assert_eq!(param_name, "gene");
            Some("THRSP!".to_string())
        }
    }

    fn import_expression_artifact(store: &ProjectStore, root: &std::path::Path) -> String {
        let source_path = root.join("expression.tsv");
        std::fs::write(&source_path, "gene\tvalue\nKRAS\t1\n").unwrap();
        store
            .import_artifact(ArtifactImportRequest {
                source_path,
                artifact_type: "ExpressionTable".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap()
            .summary
            .id
    }

    fn computed_expression_artifact(
        store: &ProjectStore,
        root: &std::path::Path,
        source_step_id: &str,
    ) -> String {
        let source_path = root.join(format!("{source_step_id}_expression.tsv"));
        std::fs::write(&source_path, "gene\tvalue\nKRAS\t2\n").unwrap();
        store
            .register_computed_artifact(ComputedArtifactRequest {
                source_path,
                artifact_type: "ExpressionTable".to_string(),
                output_name: "expression_table".to_string(),
                source_step_id: source_step_id.to_string(),
                source_run_id: "run_source".to_string(),
            })
            .unwrap()
            .summary
            .id
    }

    fn approve_marker_flow(store: &ProjectStore, artifact_id: &str) {
        store
            .approve_flow(
                FlowDraft::from_simple_yaml(&format!(
                    r#"
schema_version: agentflow.flow.v0
id: auto_flow
name: Auto apply flow
steps:
  - id: seed
    tool: analysis/marker_deepen
    reason: Existing seed analysis
    needs: []
    inputs:
      expression_table: {artifact_id}
    outputs:
      report: seed_report
"#
                ))
                .unwrap(),
                None,
            )
            .unwrap();
    }

    fn approve_auto_run_marker_flow(store: &ProjectStore, artifact_id: &str) {
        store
            .approve_flow(
                FlowDraft::from_simple_yaml(&format!(
                    r#"
schema_version: agentflow.flow.v0
id: auto_flow
name: Auto run flow
steps:
  - id: seed
    tool: analysis/marker_deepen
    reason: Existing seed analysis
    needs: []
    inputs:
      expression_table: {artifact_id}
    outputs:
      marker_report: seed_marker_report
"#
                ))
                .unwrap(),
                None,
            )
            .unwrap();
    }

    fn approve_constrained_auto_run_marker_flow(store: &ProjectStore, artifact_id: &str) {
        store
            .approve_flow(
                FlowDraft::from_simple_yaml(&format!(
                    r#"
schema_version: agentflow.flow.v0
id: auto_flow
name: Auto run flow
steps:
  - id: seed
    tool: analysis/marker_deepen
    reason: Existing seed analysis
    needs: []
    inputs:
      expression_table: {artifact_id}
    params:
      gene: TP53
    outputs:
      marker_report: seed_marker_report
"#
                ))
                .unwrap(),
                None,
            )
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
    fn apply_lifecycle_transitions_provisional_proposed_to_under_test() {
        let (path, store) = init_project("apply-lifecycle");
        let hypothesis_id = record_hypothesis(&store, "Weak support enters testing");
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );

        let report = store
            .run_cycle_with_apply_config(ApplyConfig {
                apply: true,
                auto_run: false,
                flow: None,
                max_apply: 5,
                propose_synth: false,
            })
            .unwrap();

        assert_eq!(report.outcome, CycleOutcome::Advanced);
        assert_eq!(report.raised_decisions.len(), 0);
        assert_eq!(
            report.applied,
            vec![AppliedAction::LifecycleTransition {
                hypothesis_id: hypothesis_id.clone(),
                to: "under_test".to_string(),
            }]
        );
        let hypothesis = store.inspect_hypothesis(&hypothesis_id).unwrap();
        assert_eq!(hypothesis.status, HypothesisStatus::UnderTest);
        assert_eq!(hypothesis.confidence, Confidence::Low);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn apply_with_flow_applies_deepen_graph_patch() {
        let (path, store) = init_project("apply-graph-patch");
        register_marker_tool(&store);
        let artifact_id = import_expression_artifact(&store, &path);
        approve_marker_flow(&store, &artifact_id);
        let hypothesis_id = record_hypothesis(&store, "Marker evidence needs deeper validation");
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );

        let report = store
            .run_cycle_with_apply_config(ApplyConfig {
                apply: true,
                auto_run: false,
                flow: Some("auto_flow".to_string()),
                max_apply: 5,
                propose_synth: false,
            })
            .unwrap();

        assert!(report.applied.iter().any(|action| matches!(
            action,
            AppliedAction::LifecycleTransition { hypothesis_id: id, .. } if id == &hypothesis_id
        )));
        let graph_action = report
            .applied
            .iter()
            .find_map(|action| match action {
                AppliedAction::GraphPatchApplied {
                    flow_id,
                    patch_id,
                    step_id,
                } => Some((flow_id, patch_id, step_id)),
                _ => None,
            })
            .unwrap();
        assert_eq!(graph_action.0, "auto_flow");
        assert_eq!(graph_action.2, "step_marker_deepen");

        let flow = store.inspect_flow("auto_flow").unwrap();
        assert!(flow
            .steps
            .iter()
            .any(|step| step.local_id == "step_marker_deepen"));
        let patches = store.list_graph_patches("auto_flow").unwrap();
        assert!(patches
            .iter()
            .any(|patch| patch.id == *graph_action.1 && patch.status == "applied"));
        assert!(report
            .to_json()
            .contains("\"type\":\"graph_patch_applied\""));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn auto_run_default_off_does_not_run_applied_step_or_link_evidence() {
        let (path, store) = init_project("auto-run-default-off");
        let script = write_auto_run_marker_script(&path, false);
        register_exploratory_marker_tool(&store, &script);
        let artifact_id = import_expression_artifact(&store, &path);
        approve_auto_run_marker_flow(&store, &artifact_id);
        let hypothesis_id = record_hypothesis(
            &store,
            "Marker THRSP evidence requires deeper pathway validation",
        );
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );

        let report = store
            .run_cycle_with_apply_config(ApplyConfig {
                apply: true,
                auto_run: false,
                flow: Some("auto_flow".to_string()),
                max_apply: 5,
                propose_synth: false,
            })
            .unwrap();

        assert!(report.applied.iter().any(|action| matches!(
            action,
            AppliedAction::GraphPatchApplied { step_id, .. } if step_id == "step_marker_deepen"
        )));
        assert!(!report
            .applied
            .iter()
            .any(|action| matches!(action, AppliedAction::StepRun { .. })));
        assert!(store.list_observations().unwrap().is_empty());
        assert!(!store
            .evidence_for(&hypothesis_id)
            .unwrap()
            .iter()
            .any(|link| link.note == "auto-run"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn auto_run_runs_applied_step_and_raises_stance_assessment_decision() {
        let (path, store) = init_project("auto-run-raises-stance-assessment");
        let script = write_auto_run_marker_script(&path, false);
        register_exploratory_marker_tool(&store, &script);
        let artifact_id = import_expression_artifact(&store, &path);
        approve_auto_run_marker_flow(&store, &artifact_id);
        let statement = "Marker THRSP evidence requires deeper pathway validation";
        let hypothesis_id = record_hypothesis(&store, statement);
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );

        let report = store
            .run_cycle_with_apply_config(ApplyConfig {
                apply: true,
                auto_run: true,
                flow: Some("auto_flow".to_string()),
                max_apply: 5,
                propose_synth: false,
            })
            .unwrap();

        assert_eq!(report.outcome, CycleOutcome::HandedOff);
        let (step_id, observation_id) = report
            .applied
            .iter()
            .find_map(|action| match action {
                AppliedAction::StepRun {
                    step_id,
                    observation_id: Some(observation_id),
                } => Some((step_id, observation_id)),
                _ => None,
            })
            .unwrap();
        assert_eq!(step_id, "step_marker_deepen");
        let observation = store.inspect_observation(observation_id).unwrap();
        assert_eq!(
            observation.step_id.as_deref(),
            Some("step:auto_flow/step_marker_deepen")
        );
        assert_eq!(observation.kind, "marker_report");

        let evidence = store.evidence_for(&hypothesis_id).unwrap();
        assert!(!evidence.iter().any(|link| link.note == "auto-run"));
        assert!(!evidence
            .iter()
            .any(|link| link.observation_id.as_deref() == Some(observation_id.as_str())));

        assert_eq!(report.raised_decisions.len(), 1);
        let point = &report.raised_decisions[0];
        assert_eq!(point.kind, DecisionKind::StanceAssessment);
        assert_eq!(point.recommendation, 2);
        assert_eq!(point.options.len(), 3);
        assert_eq!(point.options[0].label, "supports — 该发现支持假设");
        assert_eq!(point.options[1].label, "contradicts — 反对假设");
        assert_eq!(
            point.options[2].label,
            "inconclusive — 暂无法判定/需更多证据"
        );
        assert!(point.digest.contains("产出真实发现"));
        assert!(point.digest.contains(&observation.summary));
        assert!(point.digest.contains(statement));
        assert!(point.digest.contains(observation_id));
        assert!(point
            .digest
            .contains(&format!("evidence link --hypothesis {hypothesis_id}")));
        assert!(point
            .digest
            .contains(&format!("--observation {observation_id}")));
        assert!(point.digest.contains("--stance supports|contradicts"));
        assert!(point.digest.contains("--grade observed"));
        assert!(report.to_json().contains("\"type\":\"step_run\""));
        assert!(report.to_json().contains("\"observation_id\":\""));
        assert!(report.to_json().contains("\"kind\":\"stance_assessment\""));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn auto_run_skips_duplicate_pending_stance_assessment_for_observation() {
        let (path, store) = init_project("auto-run-stance-dedup");
        let script = write_auto_run_marker_script(&path, false);
        register_exploratory_marker_tool(&store, &script);
        let artifact_id = import_expression_artifact(&store, &path);
        approve_auto_run_marker_flow(&store, &artifact_id);
        let hypothesis_id = record_hypothesis(
            &store,
            "Marker THRSP evidence requires deeper pathway validation",
        );
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );

        let report = store
            .run_cycle_with_apply_config(ApplyConfig {
                apply: true,
                auto_run: true,
                flow: Some("auto_flow".to_string()),
                max_apply: 5,
                propose_synth: false,
            })
            .unwrap();
        assert_eq!(report.raised_decisions.len(), 1);
        let observation_id = report
            .applied
            .iter()
            .find_map(|action| match action {
                AppliedAction::StepRun {
                    observation_id: Some(observation_id),
                    ..
                } => Some(observation_id.as_str()),
                _ => None,
            })
            .unwrap();

        let mut raised_decisions = Vec::new();
        store
            .raise_stance_assessment_for_observation(
                &hypothesis_id,
                "step_marker_deepen",
                observation_id,
                &mut raised_decisions,
            )
            .unwrap();

        assert!(raised_decisions.is_empty());
        assert_eq!(
            store
                .pending_decision_points()
                .unwrap()
                .into_iter()
                .filter(|point| point.kind == DecisionKind::StanceAssessment)
                .count(),
            1
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn auto_run_step_failure_is_recorded_without_interrupting_cycle() {
        let (path, store) = init_project("auto-run-failure-continues");
        let script = write_auto_run_marker_script(&path, true);
        register_exploratory_marker_tool(&store, &script);
        let artifact_id = import_expression_artifact(&store, &path);
        approve_auto_run_marker_flow(&store, &artifact_id);
        let hypothesis_id = record_hypothesis(
            &store,
            "Marker THRSP evidence requires deeper pathway validation",
        );
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );

        let report = store
            .run_cycle_with_apply_config(ApplyConfig {
                apply: true,
                auto_run: true,
                flow: Some("auto_flow".to_string()),
                max_apply: 5,
                propose_synth: false,
            })
            .unwrap();

        assert!(report.applied.iter().any(|action| matches!(
            action,
            AppliedAction::GraphPatchApplied { step_id, .. } if step_id == "step_marker_deepen"
        )));
        assert!(report.applied.iter().any(|action| matches!(
            action,
            AppliedAction::StepRun {
                step_id,
                observation_id: None,
            } if step_id == "step_marker_deepen"
        )));
        assert_eq!(report.apply_failures.len(), 1);
        assert_eq!(report.apply_failures[0].hypothesis_id, hypothesis_id);
        assert!(report.apply_failures[0].reason.contains("auto-run step"));
        assert!(store.list_observations().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn apply_graph_patch_failures_are_recorded_and_later_proposals_continue() {
        let (path, store) = init_project("apply-graph-failure-continues");
        register_marker_tool(&store);
        let expression_id = computed_expression_artifact(&store, &path, "producer_outside_flow");
        approve_marker_flow(&store, &expression_id);
        let failing_hypothesis =
            record_hypothesis(&store, "Marker evidence needs deeper validation");
        link_evidence(
            &store,
            &failing_hypothesis,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support gives this branch medium confidence.",
        );
        let succeeding_hypothesis =
            record_hypothesis(&store, "Marker pathway follow-up needs deeper validation");
        link_evidence(
            &store,
            &succeeding_hypothesis,
            EvidenceGrade::Hypothesis,
            Stance::Supports,
            "Hypothesis-grade evidence keeps this branch lower priority.",
        );

        let report = store
            .run_cycle_with_apply_config(ApplyConfig {
                apply: true,
                auto_run: false,
                flow: Some("auto_flow".to_string()),
                max_apply: 10,
                propose_synth: false,
            })
            .unwrap();

        assert_eq!(report.branch_proposals.len(), 2);
        assert_eq!(report.apply_failures.len(), 2);
        assert_eq!(report.apply_failures[0].hypothesis_id, failing_hypothesis);
        assert_eq!(
            report.apply_failures[1].hypothesis_id,
            succeeding_hypothesis
        );
        assert!(report.apply_failures[0]
            .reason
            .contains("needs unknown step producer_outside_flow"));
        let flow = store.inspect_flow("auto_flow").unwrap();
        assert!(!flow
            .steps
            .iter()
            .any(|step| step.local_id == "step_marker_deepen"));
        assert!(report.to_json().contains("\"apply_failures\":[{"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn apply_does_not_auto_land_strong_or_abandon_decisions() {
        let (path, store) = init_project("apply-strong-abandon");
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

        let report = store
            .run_cycle_with_apply_config(ApplyConfig {
                apply: true,
                auto_run: false,
                flow: Some("unused_flow".to_string()),
                max_apply: 5,
                propose_synth: false,
            })
            .unwrap();

        assert_eq!(report.outcome, CycleOutcome::HandedOff);
        assert!(report.applied.is_empty());
        assert!(report
            .raised_decisions
            .iter()
            .any(|point| point.kind == DecisionKind::DeepenOrStop));
        assert!(report
            .raised_decisions
            .iter()
            .any(|point| point.kind == DecisionKind::GoalMutation));
        assert_eq!(
            store.inspect_hypothesis(&hypothesis_id).unwrap().status,
            HypothesisStatus::Proposed
        );
        assert_eq!(event_count(&store, "hypothesis.transitioned"), 0);
        assert_eq!(event_count(&store, "graph_patch_proposed"), 0);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn max_apply_one_raises_second_candidate_through_budget_policy() {
        let (path, store) = init_project("apply-max-one");
        let first = record_hypothesis(&store, "First weak support");
        let second = record_hypothesis(&store, "Second weak support");
        for hypothesis_id in [&first, &second] {
            link_evidence(
                &store,
                hypothesis_id,
                EvidenceGrade::LiteratureSupported,
                Stance::Supports,
                "Literature support alone is provisional.",
            );
        }

        let report = store
            .run_cycle_with_apply_config(ApplyConfig {
                apply: true,
                auto_run: false,
                flow: None,
                max_apply: 1,
                propose_synth: false,
            })
            .unwrap();

        assert_eq!(report.outcome, CycleOutcome::HandedOff);
        assert_eq!(report.applied.len(), 1);
        assert_eq!(
            store.inspect_hypothesis(&first).unwrap().status,
            HypothesisStatus::UnderTest
        );
        assert_eq!(
            store.inspect_hypothesis(&second).unwrap().status,
            HypothesisStatus::Proposed
        );
        assert!(report
            .raised_decisions
            .iter()
            .any(|point| point.kind == DecisionKind::BudgetThreshold
                && point.digest.contains(&second)));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn trace_revert_to_cycle_checkpoint_rolls_back_auto_apply() {
        let (path, store) = init_project("apply-revert");
        let hypothesis_id = record_hypothesis(&store, "Weak support can be reverted");
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );

        let report = store
            .run_cycle_with_apply_config(ApplyConfig {
                apply: true,
                auto_run: false,
                flow: None,
                max_apply: 5,
                propose_synth: false,
            })
            .unwrap();
        assert_eq!(
            store.inspect_hypothesis(&hypothesis_id).unwrap().status,
            HypothesisStatus::UnderTest
        );

        store.revert_to(&report.checkpoint_id).unwrap();

        let reverted = store.inspect_hypothesis(&hypothesis_id).unwrap();
        assert_eq!(reverted.status, HypothesisStatus::Proposed);
        assert!(store.latest_verdict_for(&hypothesis_id).unwrap().is_none());

        let _ = std::fs::remove_dir_all(path);
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
        assert!(report
            .branch_proposals
            .iter()
            .all(|proposal| matches!(&proposal.decision.action, BranchAction::Deepen { .. })));
        assert!(report
            .branch_proposals
            .iter()
            .all(|proposal| proposal.matched_tool.is_none()));
        assert!(report
            .branch_proposals
            .iter()
            .all(|proposal| proposal.drafted_step.is_none()));
        assert_eq!(event_count(&store, "argument.verdict_rendered"), 2);
        assert_eq!(event_count(&store, "agent.cycle_completed"), 1);
        assert!(report.to_json().contains("\"outcome\":\"advanced\""));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn propose_synth_raises_tool_gap_for_unmatched_branch() {
        let (path, store) = init_project("propose-synth-tool-gap");
        let hypothesis_id = record_hypothesis(&store, "Weak support needs custom validation");
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );

        let report = store
            .run_cycle_with_apply_config(ApplyConfig {
                apply: false,
                auto_run: false,
                flow: None,
                max_apply: 5,
                propose_synth: true,
            })
            .unwrap();

        assert_eq!(report.outcome, CycleOutcome::HandedOff);
        assert_eq!(report.branch_proposals.len(), 1);
        assert!(report.branch_proposals[0].matched_tool.is_none());
        assert_eq!(report.raised_decisions.len(), 1);
        let point = &report.raised_decisions[0];
        assert_eq!(point.kind, DecisionKind::ToolGap);
        assert_eq!(point.recommendation, 0);
        assert!(point.digest.contains("§15 决策痕迹"));
        assert!(point
            .digest
            .contains("能力需求 = Weak support needs custom validation"));
        assert!(point.digest.contains("无注册工具匹配"));
        assert!(point.digest.contains("agentflow synth"));
        assert!(point.digest.contains("需人类批准 + 提供验证 fixture"));
        assert!(point.digest.contains(&hypothesis_id));
        assert_eq!(point.options.len(), 3);
        assert_eq!(
            point.options[0].label,
            "合成一个工具（提供 fixture 后 synth）"
        );
        assert_eq!(point.options[1].label, "注册一个已有工具");
        assert_eq!(point.options[2].label, "跳过该分支");
        assert_eq!(event_count(&store, "handoff.decision_point_raised"), 1);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn propose_synth_default_off_does_not_raise_for_unmatched_branch() {
        let (path, store) = init_project("propose-synth-default-off");
        let hypothesis_id = record_hypothesis(&store, "Weak support stays proposal only");
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );

        let report = store
            .run_cycle_with_apply_config(ApplyConfig {
                apply: false,
                auto_run: false,
                flow: None,
                max_apply: 5,
                propose_synth: false,
            })
            .unwrap();

        assert_eq!(report.outcome, CycleOutcome::Advanced);
        assert_eq!(report.raised_decisions.len(), 0);
        assert_eq!(report.branch_proposals.len(), 1);
        assert!(report.branch_proposals[0].matched_tool.is_none());
        assert_eq!(event_count(&store, "handoff.decision_point_raised"), 0);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn propose_synth_dedups_pending_tool_gap_for_same_hypothesis() {
        let (path, store) = init_project("propose-synth-dedup");
        let hypothesis_id = record_hypothesis(&store, "Weak support needs one custom validator");
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );
        let config = ApplyConfig {
            apply: false,
            auto_run: false,
            flow: None,
            max_apply: 5,
            propose_synth: true,
        };

        let first = store.run_cycle_with_apply_config(config.clone()).unwrap();
        let second = store.run_cycle_with_apply_config(config).unwrap();

        assert_eq!(first.raised_decisions.len(), 1);
        assert_eq!(first.raised_decisions[0].kind, DecisionKind::ToolGap);
        assert!(second.raised_decisions.is_empty());
        assert_eq!(second.branch_proposals.len(), 1);
        let pending = store.pending_decision_points().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].kind, DecisionKind::ToolGap);
        assert!(pending[0].digest.contains(&hypothesis_id));
        assert_eq!(event_count(&store, "handoff.decision_point_raised"), 1);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn propose_synth_does_not_raise_tool_gap_when_tool_matches() {
        let (path, store) = init_project("propose-synth-tool-match");
        register_marker_tool(&store);
        import_expression_artifact(&store, &path);
        let hypothesis_id =
            record_hypothesis(&store, "Marker evidence requires deeper pathway validation");
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );

        let report = store
            .run_cycle_with_apply_config(ApplyConfig {
                apply: false,
                auto_run: false,
                flow: None,
                max_apply: 5,
                propose_synth: true,
            })
            .unwrap();

        assert_eq!(report.outcome, CycleOutcome::Advanced);
        assert_eq!(report.raised_decisions.len(), 0);
        assert_eq!(
            report.branch_proposals[0].matched_tool.as_deref(),
            Some("analysis/marker_deepen")
        );
        assert_eq!(event_count(&store, "handoff.decision_point_raised"), 0);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn branch_proposal_matches_tool_and_drafts_step_from_artifacts() {
        let (path, store) = init_project("enriched-proposal");
        register_marker_tool(&store);
        let artifact_id = import_expression_artifact(&store, &path);
        let hypothesis_id =
            record_hypothesis(&store, "Marker evidence requires deeper pathway validation");
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is below the decision margin.",
        );

        let report = store.run_cycle().unwrap();

        assert_eq!(report.outcome, CycleOutcome::Advanced);
        assert_eq!(report.branch_proposals.len(), 1);
        let proposal = &report.branch_proposals[0];
        assert_eq!(proposal.decision.candidate.hypothesis_id, hypothesis_id);
        assert!(matches!(
            &proposal.decision.action,
            BranchAction::Deepen { .. }
        ));
        assert_eq!(
            proposal.matched_tool.as_deref(),
            Some("analysis/marker_deepen")
        );
        assert_eq!(proposal.matched_fit.as_deref(), Some("medium"));
        assert!(proposal
            .match_reason
            .as_deref()
            .unwrap()
            .contains("input:expression_table:ExpressionTable"));

        let step = proposal.drafted_step.as_ref().unwrap();
        assert_eq!(step.id, "step_marker_deepen");
        assert_eq!(step.tool, "analysis/marker_deepen");
        assert!(step.needs.is_empty());
        assert_eq!(
            step.inputs,
            vec![("expression_table".to_string(), artifact_id)]
        );
        assert_eq!(
            step.outputs,
            vec![(
                "report".to_string(),
                "step_marker_deepen_report".to_string()
            )]
        );
        assert!(proposal.to_json().contains("\"drafted_step\":{"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn branch_proposal_infers_needs_from_computed_artifacts() {
        let (path, store) = init_project("enriched-computed-needs");
        register_marker_tool(&store);
        let artifact_id = computed_expression_artifact(&store, &path, "producer_step");
        let hypothesis_id =
            record_hypothesis(&store, "Marker evidence requires deeper pathway validation");
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is below the decision margin.",
        );

        let report = store.run_cycle().unwrap();

        assert_eq!(report.outcome, CycleOutcome::Advanced);
        assert_eq!(report.branch_proposals.len(), 1);
        let step = report.branch_proposals[0].drafted_step.as_ref().unwrap();
        assert_eq!(
            step.inputs,
            vec![("expression_table".to_string(), artifact_id)]
        );
        assert_eq!(step.needs, vec!["producer_step".to_string()]);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn branch_proposal_uses_param_inferer_for_replace_params() {
        let (path, store) = init_project("enriched-param-infer");
        register_gene_marker_tool(&store);
        import_expression_artifact(&store, &path);
        let hypothesis_id = record_hypothesis(
            &store,
            "Marker THRSP evidence requires deeper pathway validation",
        );
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is below the decision margin.",
        );

        let report = store
            .run_cycle_with(ApplyConfig::default(), &StubParamInferer)
            .unwrap();

        let step = report.branch_proposals[0].drafted_step.as_ref().unwrap();
        assert_eq!(step.params, vec![("gene".to_string(), "THRSP".to_string())]);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn inferred_params_are_emitted_after_graph_patch_apply_and_projected() {
        let (path, store) = init_project("inferred-param-provenance");
        let script = write_auto_run_marker_script(&path, false);
        register_constrained_exploratory_marker_tool(&store, &script);
        let artifact_id = import_expression_artifact(&store, &path);
        approve_constrained_auto_run_marker_flow(&store, &artifact_id);
        let hypothesis_id = record_hypothesis(
            &store,
            "Marker THRSP evidence requires deeper pathway validation",
        );
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is below the decision margin.",
        );

        let report = store
            .run_cycle_with(
                ApplyConfig {
                    apply: true,
                    auto_run: false,
                    flow: Some("auto_flow".to_string()),
                    max_apply: 5,
                    propose_synth: false,
                },
                &StubParamInferer,
            )
            .unwrap();

        assert!(report.applied.iter().any(|action| {
            matches!(
                action,
                AppliedAction::GraphPatchApplied {
                    flow_id,
                    step_id,
                    ..
                } if flow_id == "auto_flow" && step_id == "step_marker_deepen"
            )
        }));
        assert_eq!(event_count(&store, "agent.params_inferred"), 1);
        assert_eq!(
            store
                .inferred_params_for_step("auto_flow", "step_marker_deepen")
                .unwrap(),
            vec![("gene".to_string(), "THRSP".to_string())]
        );
        assert_eq!(
            store
                .inferred_params_for_step("auto_flow", "step:auto_flow/step_marker_deepen")
                .unwrap(),
            vec![("gene".to_string(), "THRSP".to_string())]
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn stance_assessment_digest_warns_for_inferred_params() {
        let (path, store) = init_project("stance-digest-inferred-param-warning");
        let script = write_auto_run_marker_script(&path, false);
        register_constrained_exploratory_marker_tool(&store, &script);
        let artifact_id = import_expression_artifact(&store, &path);
        approve_constrained_auto_run_marker_flow(&store, &artifact_id);
        let hypothesis_id = record_hypothesis(
            &store,
            "Marker THRSP evidence requires deeper pathway validation",
        );
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is below the decision margin.",
        );

        let report = store
            .run_cycle_with(
                ApplyConfig {
                    apply: true,
                    auto_run: true,
                    flow: Some("auto_flow".to_string()),
                    max_apply: 5,
                    propose_synth: false,
                },
                &StubParamInferer,
            )
            .unwrap();

        let point = report
            .raised_decisions
            .iter()
            .find(|point| point.kind == DecisionKind::StanceAssessment)
            .unwrap();
        assert!(point.digest.contains(
            "⚠ 该结果依赖 LLM 推断的未确认参数：gene=THRSP（请人工确认参数正确再据此判定立场）"
        ));
        assert_eq!(point.options.len(), 3);
        assert_eq!(point.recommendation, 2);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn stance_assessment_digest_has_no_warning_without_inferred_params() {
        let (path, store) = init_project("stance-digest-no-inferred-param-warning");
        let script = write_auto_run_marker_script(&path, false);
        register_exploratory_marker_tool(&store, &script);
        let artifact_id = import_expression_artifact(&store, &path);
        approve_auto_run_marker_flow(&store, &artifact_id);
        let hypothesis_id = record_hypothesis(
            &store,
            "Marker THRSP evidence requires deeper pathway validation",
        );
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );

        let report = store
            .run_cycle_with_apply_config(ApplyConfig {
                apply: true,
                auto_run: true,
                flow: Some("auto_flow".to_string()),
                max_apply: 5,
                propose_synth: false,
            })
            .unwrap();

        let point = report
            .raised_decisions
            .iter()
            .find(|point| point.kind == DecisionKind::StanceAssessment)
            .unwrap();
        assert!(!point.digest.contains("LLM 推断的未确认参数"));
        assert_eq!(
            store
                .inferred_params_for_step("auto_flow", "step_marker_deepen")
                .unwrap(),
            Vec::<(String, String)>::new()
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn noop_param_inferer_keeps_replace_params() {
        let (path, store) = init_project("enriched-param-noop");
        register_gene_marker_tool(&store);
        import_expression_artifact(&store, &path);
        let hypothesis_id = record_hypothesis(
            &store,
            "Marker THRSP evidence requires deeper pathway validation",
        );
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is below the decision margin.",
        );

        let report = store
            .run_cycle_with(ApplyConfig::default(), &NoopParamInferer)
            .unwrap();

        let step = report.branch_proposals[0].drafted_step.as_ref().unwrap();
        assert_eq!(
            step.params,
            vec![("gene".to_string(), "REPLACE_gene".to_string())]
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn invalid_inferred_replace_param_stays_placeholder_and_does_not_auto_run() {
        let (path, store) = init_project("invalid-inferred-param");
        let script = write_auto_run_marker_script(&path, false);
        register_constrained_exploratory_marker_tool(&store, &script);
        let artifact_id = import_expression_artifact(&store, &path);
        approve_constrained_auto_run_marker_flow(&store, &artifact_id);
        let hypothesis_id = record_hypothesis(
            &store,
            "Marker THRSP evidence requires deeper pathway validation",
        );
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is below the decision margin.",
        );

        let report = store
            .run_cycle_with(
                ApplyConfig {
                    apply: true,
                    auto_run: true,
                    flow: Some("auto_flow".to_string()),
                    max_apply: 5,
                    propose_synth: false,
                },
                &InvalidParamInferer,
            )
            .unwrap();

        let step = report.branch_proposals[0].drafted_step.as_ref().unwrap();
        assert_eq!(
            step.params,
            vec![("gene".to_string(), "REPLACE_gene".to_string())]
        );
        assert!(!report
            .applied
            .iter()
            .any(|action| matches!(action, AppliedAction::StepRun { .. })));
        assert_eq!(report.apply_failures.len(), 1);
        assert_eq!(report.apply_failures[0].hypothesis_id, hypothesis_id);
        assert!(report.apply_failures[0]
            .reason
            .contains("missing required param gene"));
        assert!(store.list_observations().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn proposal_keywords_are_deterministic() {
        assert_eq!(
            proposal_keywords(
                "KRAS-driven KRAS response, beta! RNA-seq co-op 1234 abc ABCD fifth_sixth seventh eighth ninth tenth"
            ),
            vec![
                "kras".to_string(),
                "driven".to_string(),
                "response".to_string(),
                "beta".to_string(),
                "1234".to_string(),
                "abcd".to_string(),
                "fifth".to_string(),
                "sixth".to_string()
            ]
        );
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
