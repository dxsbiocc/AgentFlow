use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use rusqlite::params;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

use crate::argument::{
    recognized_citation, ArgumentEngine, EvidenceGrade, InconclusiveKind, RuleBasedEngine, Verdict,
    VerdictReport,
};
use crate::branch::{
    BranchAction, BranchCandidate, BranchDecision, BranchPolicy, ProposedStep, RuleBasedSelector,
};
use crate::handoff::{
    Cost, DecisionKind, DecisionPoint, DefaultPolicy, HandoffOption, InterventionPolicy, Risk,
    StepContext,
};
use crate::hypothesis::{HypothesisRequest, HypothesisStatus};
use crate::storage::{
    validate_param_value, ArtifactSummary, EventRecord, FlowDraft, FlowStepDraft, ProjectStore,
    StorageError, ToolParamSpec,
};
use crate::tool_select::{extract_stored_string_field, CapabilityQuery, Fit, ToolCandidate};

const AGENT_CYCLE_COMPLETED_EVENT: &str = "agent.cycle_completed";
const PARAMS_INFERRED_EVENT: &str = "agent.params_inferred";
const SOURCE_DISCOVERY_EVENT: &str = "agent.source_discovery";
const TOOL_SYNTHESIZED_EVENT: &str = "agent.tool_synthesized";
const TOOL_GAP_HYPOTHESIS_MARKER: &str = "hypothesis_id = ";
const STANCE_ASSESSMENT_OBSERVATION_MARKER: &str = "observation_id = ";
const SOURCE_DISCOVERY_GAP_HYPOTHESIS_MARKER: &str = "source_gap_hypothesis_id = ";
const MECHANISM_SPAWN_ORIGIN_PREFIX: &str = "mechanism-spawn:from:";

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

pub trait RelevanceScorer {
    /// Returns whether a tool's output can directly test the hypothesis conclusion,
    /// not merely whether the tool is topically related.
    fn is_relevant(
        &self,
        hypothesis_statement: &str,
        tool_ref: &str,
        tool_description: &str,
    ) -> Option<bool>;
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NoopRelevanceScorer;

impl RelevanceScorer for NoopRelevanceScorer {
    fn is_relevant(
        &self,
        _hypothesis_statement: &str,
        _tool_ref: &str,
        _tool_description: &str,
    ) -> Option<bool> {
        None
    }
}

struct CachingRelevanceScorer<'a> {
    inner: &'a dyn RelevanceScorer,
    cache: RefCell<HashMap<(String, String), Option<bool>>>,
}

impl RelevanceScorer for CachingRelevanceScorer<'_> {
    fn is_relevant(
        &self,
        hypothesis_statement: &str,
        tool_ref: &str,
        tool_description: &str,
    ) -> Option<bool> {
        let key = (tool_ref.to_string(), hypothesis_statement.to_string());
        if let Some(cached) = self.cache.borrow().get(&key) {
            return *cached;
        }

        let result = self
            .inner
            .is_relevant(hypothesis_statement, tool_ref, tool_description);
        self.cache.borrow_mut().insert(key, result);
        result
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NoopToolSynthesizer;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSynthesisOutcome {
    Registered {
        tool_ref: String,
        source_trace: Option<String>,
    },
    Rejected {
        reason: String,
        source_trace: Option<String>,
        research_gap: bool,
    },
}

impl ToolSynthesisOutcome {
    pub fn registered(tool_ref: impl Into<String>) -> Self {
        Self::Registered {
            tool_ref: tool_ref.into(),
            source_trace: None,
        }
    }

    pub fn registered_with_source_trace(
        tool_ref: impl Into<String>,
        source_trace: impl Into<String>,
    ) -> Self {
        Self::Registered {
            tool_ref: tool_ref.into(),
            source_trace: Some(source_trace.into()),
        }
    }

    pub fn rejected(reason: impl Into<String>) -> Self {
        Self::Rejected {
            reason: reason.into(),
            source_trace: None,
            research_gap: false,
        }
    }

    pub fn rejected_research_gap(reason: impl Into<String>, source_trace: Option<String>) -> Self {
        Self::Rejected {
            reason: reason.into(),
            source_trace,
            research_gap: true,
        }
    }
}

pub trait ToolSynthesizer {
    fn synthesize(
        &self,
        hypothesis_statement: &str,
        capability_need: &str,
        representative_gene: Option<&str>,
    ) -> ToolSynthesisOutcome;
}

impl ToolSynthesizer for NoopToolSynthesizer {
    fn synthesize(
        &self,
        _hypothesis_statement: &str,
        _capability_need: &str,
        _representative_gene: Option<&str>,
    ) -> ToolSynthesisOutcome {
        ToolSynthesisOutcome::rejected("auto-synth unavailable")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppliedAction {
    LifecycleTransition {
        hypothesis_id: String,
        to: String,
    },
    MechanismHypothesisSpawned {
        parent_id: String,
        child_id: String,
        statement: String,
    },
    GraphPatchApplied {
        flow_id: String,
        patch_id: String,
        step_id: String,
    },
    FlowAutoCreated {
        flow_id: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceDiscoveryOutput {
    pub hypothesis_id: String,
    pub capability_need: String,
    pub trace: String,
    pub research_gap: bool,
}

impl SourceDiscoveryOutput {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("source discovery output serializes to JSON")
    }
}

struct AppliedStepFinalization<'a> {
    action: AppliedAction,
    hypothesis_id: &'a str,
    flow_id: &'a str,
    step_id: &'a str,
    step: &'a ProposedStep,
    inferred_param_names: &'a [String],
    auto_synth_tool_ref: Option<&'a str>,
    capability_need: Option<&'a str>,
    auto_synth_source_trace: Option<&'a str>,
    auto_run: bool,
}

struct ApplyCycleOutputs<'a> {
    applied: &'a mut Vec<AppliedAction>,
    apply_failures: &'a mut Vec<ApplyFailure>,
    raised_decisions: &'a mut Vec<DecisionPoint>,
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
    #[serde(default)]
    pub source_discoveries: Vec<SourceDiscoveryOutput>,
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
            + usize::from(!self.apply_failures.is_empty())
            + usize::from(!self.source_discoveries.is_empty());
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
        if !self.source_discoveries.is_empty() {
            state.serialize_field("source_discoveries", &self.source_discoveries)?;
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
        self.run_cycle_with_scorer(config, inferer, &NoopRelevanceScorer)
    }

    pub fn run_cycle_with_scorer(
        &self,
        config: ApplyConfig,
        inferer: &dyn ParamInferer,
        scorer: &dyn RelevanceScorer,
    ) -> Result<CycleReport, StorageError> {
        self.run_cycle_inner(config, inferer, scorer, &NoopToolSynthesizer, false)
    }

    pub fn run_cycle_with_synth(
        &self,
        config: ApplyConfig,
        inferer: &dyn ParamInferer,
        scorer: &dyn RelevanceScorer,
        synthesizer: &dyn ToolSynthesizer,
    ) -> Result<CycleReport, StorageError> {
        self.run_cycle_inner(config, inferer, scorer, synthesizer, true)
    }

    fn maybe_spawn_mechanism_child(
        &self,
        candidate: &BranchCandidate,
    ) -> Result<Option<(String, String)>, StorageError> {
        let parent = self.inspect_hypothesis(&candidate.hypothesis_id)?;
        if parent.origin.starts_with(MECHANISM_SPAWN_ORIGIN_PREFIX) {
            return Ok(None);
        }

        let child_origin = format!("{MECHANISM_SPAWN_ORIGIN_PREFIX}{}", parent.id);
        if self
            .list_hypotheses()?
            .into_iter()
            .any(|hypothesis| hypothesis.origin == child_origin)
        {
            return Ok(None);
        }

        let statement = format!(
            "机制探究：哪些分子机制可解释「{}」？需要哪些可直接检验该机制的证据？",
            parent.statement.trim()
        );
        let child = self.record_hypothesis(HypothesisRequest {
            statement,
            origin: child_origin,
            related_goal_id: parent.related_goal_id.clone(),
        })?;
        Ok(Some((child.id, child.statement)))
    }

    fn run_cycle_inner(
        &self,
        config: ApplyConfig,
        inferer: &dyn ParamInferer,
        scorer: &dyn RelevanceScorer,
        synthesizer: &dyn ToolSynthesizer,
        auto_synth: bool,
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
        let mut source_discoveries = Vec::new();

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
        let cycle_scorer = CachingRelevanceScorer {
            inner: scorer,
            cache: RefCell::new(HashMap::new()),
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
                    if matches!(&decision.action, BranchAction::Spawn { .. }) {
                        if let Some((child_id, statement)) =
                            self.maybe_spawn_mechanism_child(&decision.candidate)?
                        {
                            applied.push(AppliedAction::MechanismHypothesisSpawned {
                                parent_id: decision.candidate.hypothesis_id.clone(),
                                child_id,
                                statement,
                            });
                        }
                    }
                    let (mut proposal, mut inferred_param_names) = self.enrich_branch_proposal(
                        decision,
                        &available_input_types,
                        &available,
                        inferer,
                        &cycle_scorer,
                    )?;
                    let mut auto_synth_tool_ref = None;
                    let mut synthesized_capability_need = None;
                    let mut auto_synth_source_trace = None;
                    if auto_synth && auto_synth_gap(&proposal) {
                        let flow_id = config.flow.clone().unwrap_or_else(|| {
                            auto_flow_id(&proposal.decision.candidate.hypothesis_id)
                        });
                        let ctx = StepContext {
                            cost: Cost::Moderate,
                            reversible: true,
                            equivalent_branches: self.has_equivalent_tool_branches(
                                &proposal.decision,
                                &available_input_types,
                                &cycle_scorer,
                            )?,
                            conflicts_user_premise: false,
                            mutates_goal: false,
                            near_budget: applied_budget_count(&applied) as u32 >= config.max_apply,
                        };
                        if let Some(kind) = policy.assess(&ctx) {
                            let point = self.raise_decision_point(
                                kind,
                                &graph_patch_apply_digest(&proposal.decision, &flow_id),
                                graph_patch_apply_options(
                                    &proposal.decision.candidate.hypothesis_id,
                                    &flow_id,
                                ),
                                0,
                            )?;
                            raised_decisions.push(point);
                            proposal.drafted_step = None;
                        } else {
                            let capability_need = auto_synth_capability_need(&proposal);
                            if let Some((tool_ref, step, synthesized_inferred_params)) = self
                                .reusable_synthesized_tool_for_decision(
                                    &proposal.decision,
                                    &available,
                                    inferer,
                                )?
                            {
                                proposal.matched_tool = Some(tool_ref.clone());
                                proposal.matched_fit = Some("synthesized".to_string());
                                proposal.match_reason = Some(
                                    "auto_synth: reusing runtime-gated exploratory tool"
                                        .to_string(),
                                );
                                proposal.drafted_step = Some(step);
                                inferred_param_names = synthesized_inferred_params;
                                auto_synth_tool_ref = Some(tool_ref);
                                synthesized_capability_need = Some(capability_need);
                            } else {
                                let representative_gene =
                                    infer_gene_symbol(&proposal.decision.candidate.statement);
                                match synthesizer.synthesize(
                                    &proposal.decision.candidate.statement,
                                    &capability_need,
                                    representative_gene.as_deref(),
                                ) {
                                    ToolSynthesisOutcome::Registered {
                                        tool_ref,
                                        source_trace,
                                    } => {
                                        if let Some(trace) = source_trace.as_deref() {
                                            let output = source_discovery_output(
                                                &proposal.decision.candidate.hypothesis_id,
                                                &capability_need,
                                                trace,
                                                false,
                                            );
                                            self.emit_source_discovery(&output)?;
                                            source_discoveries.push(output);
                                        }
                                        match self.draft_synthesized_step(
                                            &tool_ref,
                                            &proposal.decision,
                                            &available,
                                            inferer,
                                        ) {
                                            Ok((step, synthesized_inferred_params)) => {
                                                proposal.matched_tool = Some(tool_ref.clone());
                                                proposal.matched_fit =
                                                    Some("synthesized".to_string());
                                                proposal.match_reason =
                                                    Some(auto_synth_match_reason(
                                                        "auto_synth: runtime-gated exploratory tool",
                                                        source_trace.as_deref(),
                                                    ));
                                                proposal.drafted_step = Some(step);
                                                inferred_param_names = synthesized_inferred_params;
                                                auto_synth_tool_ref = Some(tool_ref);
                                                synthesized_capability_need = Some(capability_need);
                                                auto_synth_source_trace = source_trace;
                                            }
                                            Err(error) => {
                                                proposal.drafted_step = None;
                                                apply_failures.push(ApplyFailure {
                                                    hypothesis_id: proposal
                                                        .decision
                                                        .candidate
                                                        .hypothesis_id
                                                        .clone(),
                                                    reason: format!(
                                                        "auto-synth registered tool but could not draft step: {error}"
                                                    ),
                                                });
                                            }
                                        }
                                    }
                                    ToolSynthesisOutcome::Rejected {
                                        reason,
                                        source_trace,
                                        research_gap,
                                    } => {
                                        if let Some(trace) = source_trace.as_deref() {
                                            let output = source_discovery_output(
                                                &proposal.decision.candidate.hypothesis_id,
                                                &capability_need,
                                                trace,
                                                research_gap,
                                            );
                                            self.emit_source_discovery(&output)?;
                                            source_discoveries.push(output);
                                        }
                                        proposal.drafted_step = None;
                                        let failure_reason = auto_synth_failure_reason(
                                            &reason,
                                            source_trace.as_deref(),
                                        );
                                        apply_failures.push(ApplyFailure {
                                            hypothesis_id: proposal
                                                .decision
                                                .candidate
                                                .hypothesis_id
                                                .clone(),
                                            reason: failure_reason,
                                        });
                                        if research_gap
                                            && !self.has_pending_source_discovery_gap(
                                                &proposal.decision.candidate.hypothesis_id,
                                            )?
                                        {
                                            let point = self.raise_decision_point(
                                                DecisionKind::FundamentalGap,
                                                &auto_synth_research_gap_digest(
                                                    &proposal.decision.candidate.hypothesis_id,
                                                    &proposal.decision.candidate.statement,
                                                    &reason,
                                                    source_trace.as_deref(),
                                                ),
                                                auto_synth_research_gap_options(
                                                    &proposal.decision.candidate.hypothesis_id,
                                                ),
                                                0,
                                            )?;
                                            raised_decisions.push(point);
                                        }
                                    }
                                }
                            }
                        }
                    }
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
                    let should_apply_step = config.apply;
                    if should_apply_step {
                        if let Some(step) = proposal.drafted_step.as_ref() {
                            let flow_id = config.flow.clone().unwrap_or_else(|| {
                                auto_flow_id(&proposal.decision.candidate.hypothesis_id)
                            });
                            let ctx = StepContext {
                                cost: Cost::Moderate,
                                reversible: true,
                                equivalent_branches: self.has_equivalent_tool_branches(
                                    &proposal.decision,
                                    &available_input_types,
                                    &cycle_scorer,
                                )?,
                                conflicts_user_premise: false,
                                mutates_goal: false,
                                near_budget: applied_budget_count(&applied) as u32
                                    >= config.max_apply,
                            };
                            if let Some(kind) = policy.assess(&ctx) {
                                let point = self.raise_decision_point(
                                    kind,
                                    &graph_patch_apply_digest(&proposal.decision, &flow_id),
                                    graph_patch_apply_options(
                                        &proposal.decision.candidate.hypothesis_id,
                                        &flow_id,
                                    ),
                                    0,
                                )?;
                                raised_decisions.push(point);
                            } else {
                                let apply_result = if config.flow.is_some() {
                                    self.apply_branch_patch_for_proposal(
                                        &flow_id,
                                        &proposal.decision,
                                        step,
                                    )
                                } else {
                                    self.apply_auto_flow_for_proposal(
                                        &flow_id,
                                        &proposal.decision,
                                        step,
                                    )
                                };
                                match apply_result {
                                    Ok(actions) => {
                                        for action in actions {
                                            let applied_step = match &action {
                                                AppliedAction::GraphPatchApplied {
                                                    flow_id,
                                                    step_id,
                                                    ..
                                                } => Some((flow_id.clone(), step_id.clone())),
                                                AppliedAction::FlowAutoCreated { flow_id } => {
                                                    Some((flow_id.clone(), step.id.clone()))
                                                }
                                                _ => None,
                                            };
                                            if let Some((flow_id, step_id)) = applied_step {
                                                self.record_applied_step_action(
                                                    AppliedStepFinalization {
                                                        action,
                                                        hypothesis_id: &proposal
                                                            .decision
                                                            .candidate
                                                            .hypothesis_id,
                                                        flow_id: &flow_id,
                                                        step_id: &step_id,
                                                        step,
                                                        inferred_param_names: &inferred_param_names,
                                                        auto_synth_tool_ref: auto_synth_tool_ref
                                                            .as_deref(),
                                                        capability_need:
                                                            synthesized_capability_need.as_deref(),
                                                        auto_synth_source_trace:
                                                            auto_synth_source_trace.as_deref(),
                                                        auto_run: config.auto_run,
                                                    },
                                                    ApplyCycleOutputs {
                                                        applied: &mut applied,
                                                        apply_failures: &mut apply_failures,
                                                        raised_decisions: &mut raised_decisions,
                                                    },
                                                )?;
                                            } else {
                                                applied.push(action);
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
            source_discoveries,
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

const SEMANTIC_RELEVANCE_TOP_K: usize = 3;
const SEMANTIC_RELEVANCE_REASON: &str = "relevance:semantic";
const KEYWORD_RELEVANCE_REASON: &str = "relevance:keyword";
const QUESTION_MISMATCH_DEMOTION_REASON: &str = "relevance:demoted_question_mismatch";

fn apply_semantic_relevance_to_candidates(
    store: &ProjectStore,
    candidates: &mut [ToolCandidate],
    hypothesis_statement: &str,
    scorer: &dyn RelevanceScorer,
) -> Result<bool, StorageError> {
    let mut changed = false;
    for candidate in candidates.iter_mut().take(SEMANTIC_RELEVANCE_TOP_K) {
        let is_low_candidate = candidate.fit == Fit::Low;
        let is_keyword_medium_candidate =
            candidate.fit == Fit::Medium && candidate.reason.contains(KEYWORD_RELEVANCE_REASON);
        if !is_low_candidate && !is_keyword_medium_candidate {
            continue;
        }

        let tool_description = tool_description(store, &candidate.tool_ref)?;
        match scorer.is_relevant(hypothesis_statement, &candidate.tool_ref, &tool_description) {
            Some(true) if is_low_candidate => {
                candidate.fit = Fit::Medium;
                candidate.reason =
                    append_match_reason(&candidate.reason, SEMANTIC_RELEVANCE_REASON);
                changed = true;
            }
            Some(false) if is_keyword_medium_candidate => {
                candidate.fit = Fit::Low;
                candidate.reason =
                    append_match_reason(&candidate.reason, QUESTION_MISMATCH_DEMOTION_REASON);
                changed = true;
            }
            _ => {}
        }
    }

    if changed {
        candidates.sort_by(|left, right| {
            fit_rank(right.fit)
                .cmp(&fit_rank(left.fit))
                .then_with(|| right.score.cmp(&left.score))
                .then_with(|| left.tool_ref.cmp(&right.tool_ref))
        });
    }

    Ok(changed)
}

fn tool_description(store: &ProjectStore, tool_ref: &str) -> Result<String, StorageError> {
    let inspection = store.inspect_tool(tool_ref)?;
    let spec: serde_json::Value = serde_json::from_str(&inspection.spec_json).map_err(|error| {
        StorageError::InvalidInput(format!("stored tool spec JSON is invalid: {error}"))
    })?;
    spec.get("description")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            StorageError::InvalidInput("stored tool spec is missing description".to_string())
        })
}

fn append_match_reason(existing: &str, reason: &str) -> String {
    if existing.trim().is_empty() {
        reason.to_string()
    } else {
        format!("{existing}, {reason}")
    }
}

fn fit_rank(fit: Fit) -> u8 {
    match fit {
        Fit::High => 3,
        Fit::Medium => 2,
        Fit::Low => 1,
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

    fn has_pending_source_discovery_gap(&self, hypothesis_id: &str) -> Result<bool, StorageError> {
        Ok(self
            .pending_decision_points()?
            .into_iter()
            .filter(|point| point.kind == DecisionKind::FundamentalGap)
            .filter_map(|point| source_discovery_gap_hypothesis_id(&point.digest))
            .any(|pending| pending == hypothesis_id))
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
        scorer: &dyn RelevanceScorer,
    ) -> Result<(EnrichedProposal, Vec<String>), StorageError> {
        let query = CapabilityQuery {
            desired_output_type: None,
            available_input_types: available_input_types.to_vec(),
            keywords: proposal_keywords(&decision.candidate.statement),
        };
        let mut candidates = self.match_tools(&query)?;
        apply_semantic_relevance_to_candidates(
            self,
            &mut candidates,
            &decision.candidate.statement,
            scorer,
        )?;
        let top = candidates.into_iter().next();

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
        let mut inferred_param_names = infer_replace_params(
            &mut drafted_step,
            &decision.candidate.statement,
            inferer,
            &executable.params,
        );
        if candidate.tool_ref.starts_with("synth/") {
            infer_synthesized_domain_params(
                &mut drafted_step,
                &decision.candidate.statement,
                &executable.params,
                &mut inferred_param_names,
            );
        }
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

    fn draft_synthesized_step(
        &self,
        tool_ref: &str,
        decision: &BranchDecision,
        available: &[(String, String)],
        inferer: &dyn ParamInferer,
    ) -> Result<(ProposedStep, Vec<String>), StorageError> {
        let executable = self.executable_tool(tool_ref)?;
        let mut drafted_step = self.draft_step_for(tool_ref, available)?;
        let mut inferred_param_names = infer_replace_params(
            &mut drafted_step,
            &decision.candidate.statement,
            inferer,
            &executable.params,
        );
        infer_synthesized_domain_params(
            &mut drafted_step,
            &decision.candidate.statement,
            &executable.params,
            &mut inferred_param_names,
        );
        let needs = self.infer_step_needs(&drafted_step)?;
        Ok((
            ProposedStep {
                needs,
                ..drafted_step
            },
            inferred_param_names,
        ))
    }

    fn reusable_synthesized_tool_for_decision(
        &self,
        decision: &BranchDecision,
        available: &[(String, String)],
        inferer: &dyn ParamInferer,
    ) -> Result<Option<(String, ProposedStep, Vec<String>)>, StorageError> {
        let statement = normalized_space(&decision.candidate.statement);
        if statement.is_empty() {
            return Ok(None);
        }

        for summary in self.list_tools()? {
            if summary.namespace != "synth" || summary.maturity != "exploratory" {
                continue;
            }
            let tool_ref = summary.tool_ref();
            let inspection = self.inspect_tool(&tool_ref)?;
            let description = extract_stored_string_field(&inspection.spec_json, "description")?;
            if !normalized_space(&description).contains(&statement) {
                continue;
            }
            if let Ok((step, inferred_param_names)) =
                self.draft_synthesized_step(&tool_ref, decision, available, inferer)
            {
                return Ok(Some((tool_ref, step, inferred_param_names)));
            }
        }

        Ok(None)
    }

    fn has_equivalent_tool_branches(
        &self,
        decision: &BranchDecision,
        available_input_types: &[String],
        scorer: &dyn RelevanceScorer,
    ) -> Result<bool, StorageError> {
        let query = CapabilityQuery {
            desired_output_type: None,
            available_input_types: available_input_types.to_vec(),
            keywords: proposal_keywords(&decision.candidate.statement),
        };
        let mut candidates = self.match_tools(&query)?;
        apply_semantic_relevance_to_candidates(
            self,
            &mut candidates,
            &decision.candidate.statement,
            scorer,
        )?;
        let candidate_count = candidates
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

    fn apply_auto_flow_for_proposal(
        &self,
        flow_id: &str,
        decision: &BranchDecision,
        step: &ProposedStep,
    ) -> Result<Vec<AppliedAction>, StorageError> {
        match self.inspect_flow(flow_id) {
            Ok(_) => self.apply_branch_patch_for_proposal(flow_id, decision, step),
            Err(StorageError::NotFound(_)) => {
                self.approve_flow(
                    FlowDraft {
                        schema_version: agentflow_schemas::FLOW_SCHEMA_V0.to_string(),
                        id: flow_id.to_string(),
                        name: format!("Auto flow for {}", decision.candidate.hypothesis_id),
                        steps: vec![flow_step_draft_from_proposed(step)],
                        source_text: auto_flow_source_text(flow_id, decision),
                    },
                    None,
                )?;
                Ok(vec![AppliedAction::FlowAutoCreated {
                    flow_id: flow_id.to_string(),
                }])
            }
            Err(error) => Err(error),
        }
    }

    fn record_applied_step_action(
        &self,
        finalization: AppliedStepFinalization<'_>,
        outputs: ApplyCycleOutputs<'_>,
    ) -> Result<(), StorageError> {
        self.emit_inferred_params_for_step(
            finalization.flow_id,
            finalization.step_id,
            finalization.hypothesis_id,
            finalization.step,
            finalization.inferred_param_names,
        )?;
        if let Some(tool_ref) = finalization.auto_synth_tool_ref {
            self.emit_tool_synthesized_for_step(
                finalization.flow_id,
                finalization.step_id,
                finalization.hypothesis_id,
                tool_ref,
                finalization.capability_need.unwrap_or(""),
                finalization.auto_synth_source_trace,
            )?;
        }
        outputs.applied.push(finalization.action);
        if finalization.auto_run {
            self.auto_run_applied_step(
                finalization.hypothesis_id,
                finalization.flow_id,
                finalization.step_id,
                outputs.applied,
                outputs.apply_failures,
                outputs.raised_decisions,
            )?;
        }
        Ok(())
    }

    fn emit_tool_synthesized_for_step(
        &self,
        flow_id: &str,
        step_id: &str,
        hypothesis_id: &str,
        tool_ref: &str,
        capability_need: &str,
        source_trace: Option<&str>,
    ) -> Result<(), StorageError> {
        self.append_event(EventRecord {
            flow_id: Some(flow_id.to_string()),
            step_id: Some(step_id.to_string()),
            run_id: None,
            event_type: TOOL_SYNTHESIZED_EVENT.to_string(),
            payload_json: tool_synthesized_payload_json(
                flow_id,
                step_id,
                hypothesis_id,
                tool_ref,
                capability_need,
                source_trace,
            ),
        })?;
        self.touch_project()?;
        Ok(())
    }

    fn emit_source_discovery(&self, output: &SourceDiscoveryOutput) -> Result<(), StorageError> {
        self.append_event(EventRecord {
            flow_id: None,
            step_id: None,
            run_id: None,
            event_type: SOURCE_DISCOVERY_EVENT.to_string(),
            payload_json: output.to_json(),
        })?;
        self.touch_project()?;
        Ok(())
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
        let auto_synth_provenance = self.auto_synthesized_tool_for_observation(
            observation.flow_id.as_deref(),
            observation.step_id.as_deref(),
        )?;
        let mut digest = if let Some(provenance) = auto_synth_provenance.as_ref() {
            auto_synth_stance_assessment_digest(
                step_id,
                observation_id,
                &observation.summary,
                hypothesis_id,
                &hypothesis.statement,
                &provenance.tool_ref,
                provenance.source_trace.as_deref(),
            )
        } else {
            stance_assessment_digest(
                step_id,
                observation_id,
                &observation.summary,
                hypothesis_id,
                &hypothesis.statement,
            )
        };
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

    pub(crate) fn auto_synthesized_tool_for_observation(
        &self,
        flow_id: Option<&str>,
        step_id: Option<&str>,
    ) -> Result<Option<AutoSynthProvenance>, StorageError> {
        let (Some(flow_id), Some(step_id)) = (flow_id, step_id) else {
            return Ok(None);
        };
        let mut stmt = self.connection().prepare(
            "SELECT flow_id, step_id, payload_json
             FROM events
             WHERE event_type = ?1
             ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map(params![TOOL_SYNTHESIZED_EVENT], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;

        for row in rows {
            let (event_flow_id, event_step_id, payload_json) = row?;
            let payload = tool_synthesized_payload_from_json(&payload_json)?;
            let candidate_flow_id = event_flow_id.unwrap_or(payload.flow_id);
            let candidate_step_id = event_step_id.unwrap_or(payload.step_id);
            if candidate_flow_id == flow_id && step_ids_match(flow_id, step_id, &candidate_step_id)
            {
                return Ok(Some(AutoSynthProvenance {
                    tool_ref: payload.tool_ref,
                    source_trace: payload.source_trace,
                }));
            }
        }
        Ok(None)
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ToolSynthesizedPayload {
    flow_id: String,
    step_id: String,
    hypothesis_id: String,
    tool_ref: String,
    capability_need: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_trace: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutoSynthProvenance {
    pub tool_ref: String,
    pub source_trace: Option<String>,
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

fn tool_synthesized_payload_json(
    flow_id: &str,
    step_id: &str,
    hypothesis_id: &str,
    tool_ref: &str,
    capability_need: &str,
    source_trace: Option<&str>,
) -> String {
    serde_json::to_string(&ToolSynthesizedPayload {
        flow_id: flow_id.to_string(),
        step_id: step_id.to_string(),
        hypothesis_id: hypothesis_id.to_string(),
        tool_ref: tool_ref.to_string(),
        capability_need: capability_need.to_string(),
        source_trace: source_trace.map(ToOwned::to_owned),
    })
    .expect("tool synthesized payload serializes to JSON")
}

fn source_discovery_output(
    hypothesis_id: &str,
    capability_need: &str,
    trace: &str,
    research_gap: bool,
) -> SourceDiscoveryOutput {
    SourceDiscoveryOutput {
        hypothesis_id: hypothesis_id.to_string(),
        capability_need: capability_need.to_string(),
        trace: trace.to_string(),
        research_gap,
    }
}

fn tool_synthesized_payload_from_json(
    payload_json: &str,
) -> Result<ToolSynthesizedPayload, StorageError> {
    serde_json::from_str(payload_json).map_err(|err| {
        StorageError::InvalidInput(format!("tool synthesized event has invalid payload: {err}"))
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

fn auto_synth_warning(tool_ref: &str) -> String {
    format!(
        "使用【自动合成的未验证工具 {tool_ref}】产出结果。该工具由 LLM 生成、仅过冒烟+输入敏感性检测，可能仍含编造/硬编码。请先核验工具逻辑与数据来源，再判定立场。"
    )
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
                AppliedAction::LifecycleTransition { .. }
                    | AppliedAction::GraphPatchApplied { .. }
                    | AppliedAction::FlowAutoCreated { .. }
            )
        })
        .count()
}

fn auto_flow_id(hypothesis_id: &str) -> String {
    format!("auto_{hypothesis_id}")
}

fn flow_step_draft_from_proposed(step: &ProposedStep) -> FlowStepDraft {
    FlowStepDraft {
        id: step.id.clone(),
        tool_ref: step.tool.clone(),
        needs: step.needs.clone(),
        reason: None,
        inputs: step.inputs.iter().cloned().collect(),
        params: step.params.iter().cloned().collect(),
        outputs: step.outputs.iter().cloned().collect(),
    }
}

fn auto_flow_source_text(flow_id: &str, decision: &BranchDecision) -> String {
    format!(
        "auto-generated flow {flow_id} for hypothesis {}",
        decision.candidate.hypothesis_id
    )
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

fn infer_synthesized_domain_params(
    step: &mut ProposedStep,
    hypothesis_statement: &str,
    param_specs: &BTreeMap<String, ToolParamSpec>,
    inferred_param_names: &mut Vec<String>,
) {
    for (param_name, param_value) in &mut step.params {
        let placeholder = format!("REPLACE_{param_name}");
        if param_value != &placeholder {
            continue;
        }

        let inferred = match param_name.as_str() {
            "gene" => infer_gene_symbol(hypothesis_statement),
            _ => None,
        };
        let Some(inferred) = inferred else {
            continue;
        };
        if let Some(spec) = param_specs.get(param_name) {
            if validate_param_value(spec, &inferred).is_err() {
                continue;
            }
        }

        *param_value = inferred;
        if !inferred_param_names.contains(param_name) {
            inferred_param_names.push(param_name.clone());
        }
    }
}

fn infer_gene_symbol(hypothesis_statement: &str) -> Option<String> {
    hypothesis_statement
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-')
        .map(|token| token.trim_matches('-'))
        .find(|token| is_gene_symbol_candidate(token))
        .map(ToOwned::to_owned)
}

fn normalized_space(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_gene_symbol_candidate(token: &str) -> bool {
    let token = token.trim();
    if !(2..=20).contains(&token.len()) {
        return false;
    }
    if token
        .chars()
        .any(|ch| !ch.is_ascii_alphanumeric() && ch != '-')
    {
        return false;
    }
    if !token.bytes().any(|byte| byte.is_ascii_alphabetic()) {
        return false;
    }

    let uppercase = token.to_ascii_uppercase();
    if token != uppercase {
        return false;
    }

    !matches!(
        uppercase.as_str(),
        "AUTO"
            | "SYNTH"
            | "TCGA"
            | "RNA"
            | "DNA"
            | "API"
            | "REST"
            | "LLM"
            | "AS1"
            | "AS2"
            | "AS3"
            | "L1"
            | "L2"
            | "L3"
            | "L4"
    )
}

fn auto_synth_gap(proposal: &EnrichedProposal) -> bool {
    proposal.matched_tool.is_none() || proposal.matched_fit.as_deref() == Some(Fit::Low.as_str())
}

fn auto_synth_capability_need(proposal: &EnrichedProposal) -> String {
    let statement = &proposal.decision.candidate.statement;
    match (
        proposal.matched_tool.as_deref(),
        proposal.matched_fit.as_deref(),
        proposal.match_reason.as_deref(),
    ) {
        (None, _, _) => {
            format!("能力需求 = {statement}；现状 = 无注册工具匹配（registry_match 失败）")
        }
        (Some(tool_ref), Some(fit), Some(reason)) => {
            format!(
                "能力需求 = {statement}；现状 = 最佳候选 {tool_ref} fit={fit}，不足以自动执行；匹配原因：{reason}"
            )
        }
        (Some(tool_ref), Some(fit), None) => {
            format!("能力需求 = {statement}；现状 = 最佳候选 {tool_ref} fit={fit}，不足以自动执行")
        }
        (Some(tool_ref), None, _) => {
            format!(
                "能力需求 = {statement}；现状 = 最佳候选 {tool_ref} 缺少 fit 评估，视为能力缺口"
            )
        }
    }
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
        evidence_citations(&preview.supporting),
        evidence_citations(&preview.contradicting)
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

fn auto_synth_stance_assessment_digest(
    step_id: &str,
    observation_id: &str,
    observation_summary: &str,
    hypothesis_id: &str,
    hypothesis_statement: &str,
    tool_ref: &str,
    source_trace: Option<&str>,
) -> String {
    let mut digest = format!(
        "{} 摘要：{observation_summary}。{STANCE_ASSESSMENT_OBSERVATION_MARKER}{observation_id}；请判定它对假设「{hypothesis_statement}」的立场。若仍要记录立场，运行：evidence link --hypothesis {hypothesis_id} --observation {observation_id} --stance supports|contradicts --grade hypothesis。只有人工核验工具逻辑与数据来源后，才可另行升级证据。",
        auto_synth_warning_with_step(step_id, tool_ref)
    );
    if let Some(trace) = source_trace
        .map(str::trim)
        .filter(|trace| !trace.is_empty())
    {
        digest.push('\n');
        digest.push_str(trace);
    }
    digest
}

fn auto_synth_warning_with_step(step_id: &str, tool_ref: &str) -> String {
    format!("⚠ 步骤 {step_id} {}", auto_synth_warning(tool_ref))
}

fn auto_synth_match_reason(base: &str, source_trace: Option<&str>) -> String {
    match source_trace
        .map(str::trim)
        .filter(|trace| !trace.is_empty())
    {
        Some(trace) => format!("{base}; source discovery trace: {}", one_line(trace)),
        None => base.to_string(),
    }
}

fn auto_synth_failure_reason(reason: &str, source_trace: Option<&str>) -> String {
    let base = format!("auto-synth skipped: {reason}");
    match source_trace
        .map(str::trim)
        .filter(|trace| !trace.is_empty())
    {
        Some(trace) => format!("{base}\n{trace}"),
        None => base,
    }
}

fn auto_synth_research_gap_digest(
    hypothesis_id: &str,
    hypothesis_statement: &str,
    reason: &str,
    source_trace: Option<&str>,
) -> String {
    let mut digest = format!(
        "§15 决策痕迹：{SOURCE_DISCOVERY_GAP_HYPOTHESIS_MARKER}{hypothesis_id}；未找到可访问公开数据源能直接提供回答该假设所需数据，可能是 Fundamental 研究空白（通常需前瞻 ICB 队列/响应标签/表达数据），请人类确认。假设「{hypothesis_statement}」。原因：{reason}。判决保持 inconclusive，系统未自动落 Fundamental；请人类判断是否接受研究空白、提供额外数据源或改写假设。"
    );
    if let Some(trace) = source_trace
        .map(str::trim)
        .filter(|trace| !trace.is_empty())
    {
        digest.push('\n');
        digest.push_str(trace);
    }
    digest
}

fn auto_synth_research_gap_options(hypothesis_id: &str) -> Vec<HandoffOption> {
    vec![
        HandoffOption {
            label: "确认 Fundamental 研究空白".to_string(),
            direction: format!(
                "人工确认假设 {hypothesis_id} 是 Fundamental 研究空白；如需落判决，走既有 render_verdict + self-deception gate"
            ),
            cost: Cost::Cheap,
            risk: Risk::Low,
            reversible: true,
        },
        HandoffOption {
            label: "提供数据源".to_string(),
            direction: format!("为假设 {hypothesis_id} 提供可访问公开源或本地数据后重跑"),
            cost: Cost::Moderate,
            risk: Risk::Low,
            reversible: true,
        },
        HandoffOption {
            label: "改写假设".to_string(),
            direction: format!("收窄或改写假设 {hypothesis_id} 以匹配可访问数据"),
            cost: Cost::Moderate,
            risk: Risk::Medium,
            reversible: true,
        },
    ]
}

fn source_discovery_gap_hypothesis_id(digest: &str) -> Option<String> {
    let rest = digest.split_once(SOURCE_DISCOVERY_GAP_HYPOTHESIS_MARKER)?.1;
    rest.split('；').next().map(str::trim).and_then(|id| {
        if id.is_empty() {
            None
        } else {
            Some(id.to_string())
        }
    })
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
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

fn evidence_citations(evidence: &[crate::argument::EvidenceLink]) -> String {
    if evidence.is_empty() {
        "none".to_string()
    } else {
        evidence
            .iter()
            .map(|link| {
                recognized_citation(link.source.as_deref()).unwrap_or_else(|| {
                    if link.grade == EvidenceGrade::LiteratureSupported {
                        "⚠未引用"
                    } else {
                        link.id.as_str()
                    }
                })
            })
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
    use std::cell::RefCell;
    use std::path::PathBuf;

    use rusqlite::params;

    use crate::argument::{
        ArgumentEngine, EvidenceGrade, EvidenceLink, EvidenceLinkRequest, InconclusiveKind,
        RuleBasedEngine, Stance, Verdict, VerdictReport, VerdictTag,
    };
    use crate::branch::{
        BranchAction, BranchCandidate, BranchDecision, BranchPolicy, CandidateKind, ProposedStep,
        RuleBasedSelector, SelectionMode,
    };
    use crate::handoff::DecisionKind;
    use crate::hypothesis::{Confidence, HypothesisRequest, HypothesisStatus};
    use crate::storage::{
        now_unix_seconds, ArtifactImportMode, ArtifactImportRequest, ComputedArtifactRequest,
        FlowDraft, ProjectStore, ToolSpec,
    };
    use crate::tool_select::{Fit, ToolCandidate};

    use super::{
        proposal_keywords, AppliedAction, ApplyConfig, CycleOutcome, NoopParamInferer,
        NoopRelevanceScorer, ParamInferer, RelevanceScorer, ToolSynthesisOutcome, ToolSynthesizer,
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
            (AppliedAction::FlowAutoCreated {
                flow_id: "auto_hypothesis_1".to_string(),
            })
            .to_json(),
            "{\"type\":\"flow_auto_created\",\"flow_id\":\"auto_hypothesis_1\"}"
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
            source_discoveries: Vec::new(),
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
            source_discoveries: Vec::new(),
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
    fn evidence_citations_prefers_verifiable_sources_and_marks_uncited_literature() {
        let cited = EvidenceLink {
            id: "literature_cited".to_string(),
            hypothesis_id: "hypothesis_1".to_string(),
            observation_id: None,
            source: Some(" PMID:123 ".to_string()),
            grade: EvidenceGrade::LiteratureSupported,
            stance: Stance::Supports,
            note: "Cited literature support".to_string(),
            created_at: 1,
        };
        let uncited = EvidenceLink {
            id: "literature_uncited".to_string(),
            hypothesis_id: "hypothesis_1".to_string(),
            observation_id: None,
            source: Some("trust me".to_string()),
            grade: EvidenceGrade::LiteratureSupported,
            stance: Stance::Supports,
            note: "Uncited literature support".to_string(),
            created_at: 1,
        };
        let observed = EvidenceLink {
            id: "observed_1".to_string(),
            hypothesis_id: "hypothesis_1".to_string(),
            observation_id: Some("observation_1".to_string()),
            source: None,
            grade: EvidenceGrade::Observed,
            stance: Stance::Contradicts,
            note: "Observed contradiction".to_string(),
            created_at: 1,
        };

        assert_eq!(
            super::evidence_citations(&[cited.clone(), uncited.clone(), observed.clone()]),
            "PMID:123,⚠未引用,observed_1"
        );
        assert_eq!(super::evidence_citations(&[]), "none");

        let digest = super::strong_verdict_digest(&VerdictReport {
            hypothesis_id: "hypothesis_1".to_string(),
            verdict: Verdict::Affirmed,
            confidence: Confidence::High,
            supporting: vec![cited, uncited],
            contradicting: vec![EvidenceLink {
                source: Some("DOI:10.test/against".to_string()),
                ..observed
            }],
            rationale: "Strong but needs human gate".to_string(),
        });

        assert!(digest.contains("支持证据 PMID:123,⚠未引用"));
        assert!(digest.contains("反证 DOI:10.test/against"));
        assert!(!digest.contains("literature_cited"));
        assert!(!digest.contains("literature_uncited"));
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

    fn affirm_hypothesis(store: &ProjectStore, hypothesis_id: &str) {
        link_evidence(
            store,
            hypothesis_id,
            EvidenceGrade::Observed,
            Stance::Supports,
            "Observed support reaches the rule margin.",
        );
        store
            .render_verdict(
                hypothesis_id,
                &crate::argument::RuleBasedEngine,
                Some(gate()),
            )
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

    fn write_no_input_auto_run_marker_script(root: &std::path::Path) -> PathBuf {
        let script_path = root.join("auto_run_marker_no_input.sh");
        std::fs::write(
            &script_path,
            "printf '# Marker report\nGene: THRSP\nscore: 0.61\n' > \"$AGENTFLOW_OUTPUT_MARKER_REPORT\"\n",
        )
        .unwrap();
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

    fn register_no_input_constrained_marker_tool(
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

    struct StubRelevanceScorer {
        relevant_tool: Option<&'static str>,
        fallback: Option<bool>,
        calls: RefCell<Vec<String>>,
    }

    impl StubRelevanceScorer {
        fn relevant(tool_ref: &'static str) -> Self {
            Self {
                relevant_tool: Some(tool_ref),
                fallback: Some(false),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn always(result: Option<bool>) -> Self {
            Self {
                relevant_tool: None,
                fallback: result,
                calls: RefCell::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl RelevanceScorer for StubRelevanceScorer {
        fn is_relevant(
            &self,
            hypothesis_statement: &str,
            tool_ref: &str,
            tool_description: &str,
        ) -> Option<bool> {
            assert!(hypothesis_statement.contains("THRSP"));
            assert!(!tool_description.trim().is_empty());
            self.calls.borrow_mut().push(tool_ref.to_string());
            if self.relevant_tool == Some(tool_ref) {
                Some(true)
            } else {
                self.fallback
            }
        }
    }

    struct StubToolSynthesizer<'a> {
        store: &'a ProjectStore,
        script_path: PathBuf,
        tool_name: &'static str,
        calls: RefCell<Vec<(String, String, Option<String>)>>,
        should_register: bool,
        source_trace: Option<&'static str>,
        research_gap: bool,
    }

    impl<'a> StubToolSynthesizer<'a> {
        fn registering(
            store: &'a ProjectStore,
            root: &std::path::Path,
            tool_name: &'static str,
        ) -> Self {
            let script_path = root.join(format!("{tool_name}.sh"));
            std::fs::write(
                &script_path,
                "printf '# Auto synth report\ngene: %s\nstatus: ok\n' \"$AGENTFLOW_PARAM_GENE\" > \"$AGENTFLOW_OUTPUT_RESULT\"\n",
            )
            .unwrap();
            Self {
                store,
                script_path,
                tool_name,
                calls: RefCell::new(Vec::new()),
                should_register: true,
                source_trace: None,
                research_gap: false,
            }
        }

        fn none(store: &'a ProjectStore) -> Self {
            Self {
                store,
                script_path: PathBuf::new(),
                tool_name: "unused",
                calls: RefCell::new(Vec::new()),
                should_register: false,
                source_trace: None,
                research_gap: false,
            }
        }

        fn research_gap(store: &'a ProjectStore, source_trace: &'static str) -> Self {
            Self {
                store,
                script_path: PathBuf::new(),
                tool_name: "unused",
                calls: RefCell::new(Vec::new()),
                should_register: false,
                source_trace: Some(source_trace),
                research_gap: true,
            }
        }

        fn with_source_trace(mut self, source_trace: &'static str) -> Self {
            self.source_trace = Some(source_trace);
            self
        }

        fn calls(&self) -> Vec<(String, String, Option<String>)> {
            self.calls.borrow().clone()
        }
    }

    impl ToolSynthesizer for StubToolSynthesizer<'_> {
        fn synthesize(
            &self,
            hypothesis_statement: &str,
            capability_need: &str,
            representative_gene: Option<&str>,
        ) -> ToolSynthesisOutcome {
            self.calls.borrow_mut().push((
                hypothesis_statement.to_string(),
                capability_need.to_string(),
                representative_gene.map(ToOwned::to_owned),
            ));
            if !self.should_register {
                if self.research_gap {
                    return ToolSynthesisOutcome::rejected_research_gap(
                        "未找到可访问公开数据源能为该假设提供数据，可能是真实研究空白",
                        self.source_trace.map(str::to_string),
                    );
                }
                return ToolSynthesisOutcome::rejected("stub synthesizer returned no candidate");
            }
            let command = self.script_path.display();
            let spec = ToolSpec::from_simple_yaml(&format!(
                r#"schema_version: agentflow.tool.v0
namespace: synth
name: {}
version: 0.1.0
maturity: exploratory
description: Auto-synthesized tool for hypothesis {hypothesis_statement}. Capability need {capability_need}
params:
  gene:
    type: string
    required: true
outputs:
  result:
    type: Markdown
    observer: artifact_summary
runtime:
  backend: local
  command:
    - /bin/sh
    - {command}
"#,
                self.tool_name
            ))
            .unwrap();
            let tool_ref = self.store.register_tool(spec).unwrap().tool_ref;
            match self.source_trace {
                Some(trace) => ToolSynthesisOutcome::registered_with_source_trace(tool_ref, trace),
                None => ToolSynthesisOutcome::registered(tool_ref),
            }
        }
    }

    fn register_semantic_tool(
        store: &ProjectStore,
        name: &str,
        maturity: &str,
        description: &str,
        inputs: &[(&str, &str)],
    ) {
        let input_yaml = if inputs.is_empty() {
            String::new()
        } else {
            let entries = inputs
                .iter()
                .map(|(name, type_name)| {
                    format!("  {name}:\n    type: {type_name}\n    required: true\n")
                })
                .collect::<String>();
            format!("inputs:\n{entries}")
        };
        let spec = ToolSpec::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.tool.v0
namespace: analysis
name: {name}
version: 0.1.0
maturity: {maturity}
description: {description}
{input_yaml}outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#
        ))
        .unwrap();
        store.register_tool(spec).unwrap();
    }

    fn register_semantic_rerank_tools(store: &ProjectStore) {
        register_semantic_tool(
            store,
            "score_low_current",
            "verified",
            "validation helper for unrelated prioritization",
            &[
                ("expression_table", "ExpressionTable"),
                ("cohort_table", "CohortTable"),
            ],
        );
        register_semantic_tool(
            store,
            "latent_assoc",
            "verified",
            "mechanism analysis for latent association",
            &[],
        );
        register_semantic_tool(
            store,
            "io_medium",
            "exploratory",
            "generic assay runner",
            &[("expression_table", "ExpressionTable")],
        );
    }

    fn setup_semantic_project(test_name: &str) -> (PathBuf, ProjectStore) {
        let (path, store) = init_project(test_name);
        register_semantic_rerank_tools(&store);
        import_expression_artifact(&store, &path);
        let hypothesis_id = record_hypothesis(&store, "THRSP survival mechanism needs validation");
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is below the decision margin.",
        );
        (path, store)
    }

    fn setup_keyword_relevance_project(test_name: &str) -> (PathBuf, ProjectStore) {
        let (path, store) = init_project(test_name);
        register_semantic_tool(
            &store,
            "thrsp_survival_proxy",
            "verified",
            "THRSP survival association proxy for related cohort analysis",
            &[],
        );
        let hypothesis_id = record_hypothesis(&store, "THRSP survival mechanism needs validation");
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is below the decision margin.",
        );
        (path, store)
    }

    fn setup_equivalent_keyword_relevance_project(test_name: &str) -> (PathBuf, ProjectStore) {
        let (path, store) = setup_keyword_relevance_project(test_name);
        register_semantic_tool(
            &store,
            "thrsp_axis_assoc",
            "verified",
            "THRSP survival mechanism axis association proxy for related cohort analysis",
            &[],
        );
        (path, store)
    }

    fn selected_branch_decision(store: &ProjectStore) -> BranchDecision {
        let mut decisions = store
            .select_branches(
                &RuleBasedSelector,
                &BranchPolicy {
                    explore_enabled: false,
                },
            )
            .unwrap();
        assert_eq!(decisions.len(), 1);
        decisions.remove(0)
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
    fn apply_without_flow_auto_creates_flow_runs_step_and_raises_stance_assessment() {
        let (path, store) = init_project("auto-flow-runs-stance-assessment");
        let script = write_no_input_auto_run_marker_script(&path);
        register_no_input_constrained_marker_tool(&store, &script);
        let statement = "Marker THRSP evidence requires deeper pathway validation";
        let hypothesis_id = record_hypothesis(&store, statement);
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );
        let auto_flow_id = format!("auto_{hypothesis_id}");

        let report = store
            .run_cycle_with(
                ApplyConfig {
                    apply: true,
                    auto_run: true,
                    flow: None,
                    max_apply: 5,
                    propose_synth: false,
                },
                &StubParamInferer,
            )
            .unwrap();

        assert_eq!(report.outcome, CycleOutcome::HandedOff);
        assert!(report.applied.iter().any(|action| matches!(
            action,
            AppliedAction::FlowAutoCreated { flow_id } if flow_id == &auto_flow_id
        )));
        let flow = store.inspect_flow(&auto_flow_id).unwrap();
        assert_eq!(flow.steps.len(), 1);
        assert_eq!(flow.steps[0].local_id, "step_marker_deepen");
        assert!(flow.steps[0].inputs_json.contains("{}"));

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
            Some(format!("step:{auto_flow_id}/step_marker_deepen").as_str())
        );
        assert_eq!(observation.kind, "marker_report");
        assert_eq!(
            store
                .inferred_params_for_step(&auto_flow_id, "step_marker_deepen")
                .unwrap(),
            vec![("gene".to_string(), "THRSP".to_string())]
        );

        let point = report
            .raised_decisions
            .iter()
            .find(|point| point.kind == DecisionKind::StanceAssessment)
            .unwrap();
        assert!(point.digest.contains(observation_id));
        assert!(point.digest.contains(statement));
        assert!(point.digest.contains(
            "⚠ 该结果依赖 LLM 推断的未确认参数：gene=THRSP（请人工确认参数正确再据此判定立场）"
        ));
        assert!(report.to_json().contains("\"type\":\"flow_auto_created\""));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn apply_with_existing_flow_does_not_auto_create_flow() {
        let (path, store) = init_project("flow-some-no-auto-create");
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

        assert!(!report
            .applied
            .iter()
            .any(|action| matches!(action, AppliedAction::FlowAutoCreated { .. })));
        assert_eq!(event_count(&store, "flow_approved"), 1);
        assert!(store
            .inspect_flow(&format!("auto_{hypothesis_id}"))
            .is_err());

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn apply_without_flow_budget_brake_raises_decision_instead_of_creating_flow() {
        let (path, store) = init_project("auto-flow-budget-brake");
        let script = write_no_input_auto_run_marker_script(&path);
        register_no_input_constrained_marker_tool(&store, &script);
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
            .run_cycle_with(
                ApplyConfig {
                    apply: true,
                    auto_run: true,
                    flow: None,
                    max_apply: 1,
                    propose_synth: false,
                },
                &StubParamInferer,
            )
            .unwrap();

        assert_eq!(report.outcome, CycleOutcome::HandedOff);
        assert!(report.applied.iter().any(|action| matches!(
            action,
            AppliedAction::LifecycleTransition { hypothesis_id: id, .. } if id == &hypothesis_id
        )));
        assert!(!report
            .applied
            .iter()
            .any(|action| matches!(action, AppliedAction::FlowAutoCreated { .. })));
        assert!(!report
            .applied
            .iter()
            .any(|action| matches!(action, AppliedAction::StepRun { .. })));
        assert!(report
            .raised_decisions
            .iter()
            .any(|point| point.kind == DecisionKind::BudgetThreshold));
        assert_eq!(event_count(&store, "flow_approved"), 0);
        assert!(store
            .inspect_flow(&format!("auto_{hypothesis_id}"))
            .is_err());

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn apply_without_flow_reuses_existing_auto_flow_on_same_hypothesis() {
        let (path, store) = init_project("auto-flow-idempotent");
        let script = write_no_input_auto_run_marker_script(&path);
        register_no_input_constrained_marker_tool(&store, &script);
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
        let auto_flow_id = format!("auto_{hypothesis_id}");
        let config = ApplyConfig {
            apply: true,
            auto_run: false,
            flow: None,
            max_apply: 5,
            propose_synth: false,
        };

        let first = store
            .run_cycle_with(config.clone(), &StubParamInferer)
            .unwrap();
        let second = store.run_cycle_with(config, &StubParamInferer).unwrap();

        assert!(first.applied.iter().any(|action| matches!(
            action,
            AppliedAction::FlowAutoCreated { flow_id } if flow_id == &auto_flow_id
        )));
        assert!(!second
            .applied
            .iter()
            .any(|action| matches!(action, AppliedAction::FlowAutoCreated { .. })));
        assert_eq!(event_count(&store, "flow_approved"), 1);
        let flow = store.inspect_flow(&auto_flow_id).unwrap();
        assert_eq!(
            flow.steps
                .iter()
                .filter(|step| step.local_id == "step_marker_deepen")
                .count(),
            1
        );

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
    fn spawn_branch_creates_proposed_mechanism_child() {
        let (path, store) = init_project("mechanism-spawn");
        let statement = "Observed support should seed a mechanism question";
        let hypothesis_id = record_hypothesis(&store, statement);
        affirm_hypothesis(&store, &hypothesis_id);

        let report = store.run_cycle().unwrap();

        let (child_id, child_statement) = report
            .applied
            .iter()
            .find_map(|action| match action {
                AppliedAction::MechanismHypothesisSpawned {
                    parent_id,
                    child_id,
                    statement,
                } if parent_id == &hypothesis_id => Some((child_id.clone(), statement.clone())),
                _ => None,
            })
            .unwrap();
        assert!(matches!(
            &report.branch_proposals[0].decision.action,
            BranchAction::Spawn { .. }
        ));
        assert_eq!(report.outcome, CycleOutcome::HandedOff);
        assert_eq!(report.branch_proposals.len(), 1);
        assert_eq!(store.list_hypotheses().unwrap().len(), 2);

        let child = store.inspect_hypothesis(&child_id).unwrap();
        assert_eq!(child.status, HypothesisStatus::Proposed);
        assert_eq!(child.related_goal_id, "goal_agent");
        assert_eq!(
            child.origin,
            format!("{}{}", super::MECHANISM_SPAWN_ORIGIN_PREFIX, hypothesis_id)
        );
        assert_eq!(child.statement, child_statement);
        assert!(child.statement.contains("机制探究"));
        assert!(child.statement.contains(statement));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn mechanism_spawn_is_idempotent_per_parent_across_cycles() {
        let (path, store) = init_project("mechanism-spawn-idempotent");
        let hypothesis_id = record_hypothesis(&store, "Observed support should spawn once");
        affirm_hypothesis(&store, &hypothesis_id);

        let first = store.run_cycle().unwrap();
        let second = store.run_cycle().unwrap();

        assert!(first.applied.iter().any(|action| matches!(
            action,
            AppliedAction::MechanismHypothesisSpawned { parent_id, .. }
                if parent_id == &hypothesis_id
        )));
        assert!(!second.applied.iter().any(|action| matches!(
            action,
            AppliedAction::MechanismHypothesisSpawned { parent_id, .. }
                if parent_id == &hypothesis_id
        )));
        let child_origin = format!("{}{}", super::MECHANISM_SPAWN_ORIGIN_PREFIX, hypothesis_id);
        let children = store
            .list_hypotheses()
            .unwrap()
            .into_iter()
            .filter(|hypothesis| hypothesis.origin == child_origin)
            .collect::<Vec<_>>();
        assert_eq!(children.len(), 1);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn mechanism_spawn_child_does_not_spawn_grandchild() {
        let (path, store) = init_project("mechanism-spawn-no-chain");
        let hypothesis_id = record_hypothesis(&store, "Observed support should stop at one child");
        affirm_hypothesis(&store, &hypothesis_id);
        let first = store.run_cycle().unwrap();
        let child_id = first
            .applied
            .iter()
            .find_map(|action| match action {
                AppliedAction::MechanismHypothesisSpawned { child_id, .. } => {
                    Some(child_id.clone())
                }
                _ => None,
            })
            .unwrap();

        affirm_hypothesis(&store, &child_id);
        let second = store.run_cycle().unwrap();

        assert!(!second.applied.iter().any(|action| matches!(
            action,
            AppliedAction::MechanismHypothesisSpawned { parent_id, .. }
                if parent_id == &child_id
        )));
        let grandchild_origin = format!("{}{}", super::MECHANISM_SPAWN_ORIGIN_PREFIX, child_id);
        assert!(store
            .list_hypotheses()
            .unwrap()
            .iter()
            .all(|hypothesis| hypothesis.origin.as_str() != grandchild_origin));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn deepen_branch_does_not_spawn_mechanism_child() {
        let (path, store) = init_project("mechanism-spawn-deepen-none");
        let hypothesis_id = record_hypothesis(&store, "Weak support needs deeper validation");
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );

        let report = store.run_cycle().unwrap();

        assert!(matches!(
            &report.branch_proposals[0].decision.action,
            BranchAction::Deepen { .. }
        ));
        assert!(report
            .applied
            .iter()
            .all(|action| !matches!(action, AppliedAction::MechanismHypothesisSpawned { .. })));
        assert_eq!(store.list_hypotheses().unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_success_registers_runs_marks_provenance_and_warns_l4() {
        let (path, store) = init_project("auto-synth-success");
        let statement = "Auto synth THRSP pathway validation needs custom validation";
        let hypothesis_id = record_hypothesis(&store, statement);
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );
        let synthesizer =
            StubToolSynthesizer::registering(&store, &path, "auto_synth_thrsp_validation");

        let report = store
            .run_cycle_with_synth(
                ApplyConfig {
                    apply: true,
                    auto_run: true,
                    flow: None,
                    max_apply: 5,
                    propose_synth: false,
                },
                &NoopParamInferer,
                &NoopRelevanceScorer,
                &synthesizer,
            )
            .unwrap();

        assert_eq!(synthesizer.calls().len(), 1);
        assert_eq!(synthesizer.calls()[0].0, statement);
        assert!(synthesizer.calls()[0].1.contains("无注册工具匹配"));
        assert_eq!(synthesizer.calls()[0].2.as_deref(), Some("THRSP"));
        assert_eq!(report.branch_proposals.len(), 1);
        let proposal = &report.branch_proposals[0];
        assert_eq!(
            proposal.matched_tool.as_deref(),
            Some("synth/auto_synth_thrsp_validation")
        );
        assert_eq!(proposal.matched_fit.as_deref(), Some("synthesized"));
        assert!(proposal
            .match_reason
            .as_deref()
            .unwrap()
            .contains("auto_synth"));
        let step = proposal.drafted_step.as_ref().unwrap();
        assert_eq!(step.params, vec![("gene".to_string(), "THRSP".to_string())]);
        let auto_flow_id = format!("auto_{hypothesis_id}");
        assert!(report.applied.iter().any(|action| matches!(
            action,
            AppliedAction::FlowAutoCreated { flow_id } if flow_id == &auto_flow_id
        )));
        let observation_id = report
            .applied
            .iter()
            .find_map(|action| match action {
                AppliedAction::StepRun {
                    observation_id: Some(observation_id),
                    ..
                } => Some(observation_id.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            store
                .inferred_params_for_step(&auto_flow_id, "step_auto_synth_thrsp_validation")
                .unwrap(),
            vec![("gene".to_string(), "THRSP".to_string())]
        );
        assert_eq!(event_count(&store, "agent.tool_synthesized"), 1);

        let point = report
            .raised_decisions
            .iter()
            .find(|point| point.kind == DecisionKind::StanceAssessment)
            .unwrap();
        assert!(!point.digest.contains("产出真实发现"));
        assert!(point.digest.contains("自动合成的未验证工具"));
        assert!(point.digest.contains("冒烟+输入敏感性检测"));
        assert!(point.digest.contains("可能仍含编造/硬编码"));
        assert!(point.digest.contains("请先核验工具逻辑与数据来源"));

        let linked = store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: hypothesis_id.clone(),
                observation_id: Some(observation_id),
                source: None,
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: "Human accepted auto synth output for this fixture.".to_string(),
            })
            .unwrap();
        assert_eq!(linked.grade, EvidenceGrade::Hypothesis);
        let verdict =
            RuleBasedEngine.render(&hypothesis_id, &store.evidence_for(&hypothesis_id).unwrap());
        assert!(matches!(
            verdict.verdict,
            Verdict::Inconclusive(InconclusiveKind::Provisional { .. })
        ));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_source_trace_is_visible_in_l4_digest() {
        let (path, store) = init_project("auto-synth-source-trace-l4");
        let statement = "MID1IP1 immunotherapy response needs public ICB data";
        let hypothesis_id = record_hypothesis(&store, statement);
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );
        let synthesizer = StubToolSynthesizer::registering(
            &store,
            &path,
            "auto_synth_mid1ip1_source",
        )
        .with_source_trace(
            "SOURCE DISCOVERY TRACE\n- NCBI GEO: viable; probe returned MID1IP1 immunotherapy response cohort",
        );

        let report = store
            .run_cycle_with_synth(
                ApplyConfig {
                    apply: true,
                    auto_run: true,
                    flow: None,
                    max_apply: 5,
                    propose_synth: false,
                },
                &NoopParamInferer,
                &NoopRelevanceScorer,
                &synthesizer,
            )
            .unwrap();

        let point = report
            .raised_decisions
            .iter()
            .find(|point| point.kind == DecisionKind::StanceAssessment)
            .unwrap();
        assert!(point.digest.contains("SOURCE DISCOVERY TRACE"));
        assert!(point.digest.contains("NCBI GEO"));
        assert!(point
            .digest
            .contains("MID1IP1 immunotherapy response cohort"));
        assert!(point.digest.contains("自动合成的未验证工具"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_reuses_registered_synth_tool_for_same_hypothesis_without_resynthesis() {
        let (path, store) = init_project("auto-synth-reuse-dedup");
        let statement = "Auto synth THRSP pathway validation needs custom validation";
        let hypothesis_id = record_hypothesis(&store, statement);
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );
        let first_synthesizer =
            StubToolSynthesizer::registering(&store, &path, "auto_synth_thrsp_validation");

        let first = store
            .run_cycle_with_synth(
                ApplyConfig {
                    apply: false,
                    auto_run: false,
                    flow: None,
                    max_apply: 5,
                    propose_synth: false,
                },
                &NoopParamInferer,
                &NoopRelevanceScorer,
                &first_synthesizer,
            )
            .unwrap();
        assert_eq!(first_synthesizer.calls().len(), 1);
        assert_eq!(
            first.branch_proposals[0].matched_tool.as_deref(),
            Some("synth/auto_synth_thrsp_validation")
        );

        let second_synthesizer = StubToolSynthesizer::none(&store);
        let second = store
            .run_cycle_with_synth(
                ApplyConfig {
                    apply: false,
                    auto_run: false,
                    flow: None,
                    max_apply: 5,
                    propose_synth: false,
                },
                &NoopParamInferer,
                &NoopRelevanceScorer,
                &second_synthesizer,
            )
            .unwrap();

        assert!(second_synthesizer.calls().is_empty());
        assert_eq!(
            second.branch_proposals[0].matched_tool.as_deref(),
            Some("synth/auto_synth_thrsp_validation")
        );
        assert_eq!(
            second.branch_proposals[0]
                .drafted_step
                .as_ref()
                .unwrap()
                .params,
            vec![("gene".to_string(), "THRSP".to_string())]
        );
        assert!(second.apply_failures.is_empty());
        assert_eq!(event_count(&store, "tool_registered"), 1);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_none_path_does_not_register_or_run() {
        let (path, store) = init_project("auto-synth-none");
        let hypothesis_id =
            record_hypothesis(&store, "Auto synth missing backend needs custom validation");
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );
        let synthesizer = StubToolSynthesizer::none(&store);

        let report = store
            .run_cycle_with_synth(
                ApplyConfig {
                    apply: false,
                    auto_run: false,
                    flow: None,
                    max_apply: 5,
                    propose_synth: false,
                },
                &NoopParamInferer,
                &NoopRelevanceScorer,
                &synthesizer,
            )
            .unwrap();

        assert_eq!(synthesizer.calls().len(), 1);
        assert_eq!(report.branch_proposals.len(), 1);
        assert!(report.branch_proposals[0].matched_tool.is_none());
        assert!(report.branch_proposals[0].drafted_step.is_none());
        assert!(report
            .apply_failures
            .iter()
            .any(|failure| failure.reason.contains("auto-synth skipped")));
        assert!(report.applied.is_empty());
        assert!(report.raised_decisions.is_empty());
        assert_eq!(event_count(&store, "tool_registered"), 0);
        assert_eq!(event_count(&store, "agent.tool_synthesized"), 0);
        assert!(store.list_observations().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_research_gap_raises_honest_handoff_digest() {
        let (path, store) = init_project("auto-synth-research-gap");
        let hypothesis_id = record_hypothesis(
            &store,
            "MID1IP1 immunotherapy response needs public ICB data",
        );
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );
        let synthesizer = StubToolSynthesizer::research_gap(
            &store,
            "QUESTION DATA REQUIREMENTS\nrequired_data: ICB treatment cohort + response labels + gene expression\nSOURCE DISCOVERY TRACE\n- cBioPortal: related-but-insufficient; has_required_data=no; reason=related expression/survival data but no ICB response labels\n- NCBI GEO: probed, empty\n代理分析但不直接回答本问题",
        );

        let report = store
            .run_cycle_with_synth(
                ApplyConfig {
                    apply: false,
                    auto_run: false,
                    flow: None,
                    max_apply: 5,
                    propose_synth: false,
                },
                &NoopParamInferer,
                &NoopRelevanceScorer,
                &synthesizer,
            )
            .unwrap();

        assert_eq!(synthesizer.calls().len(), 1);
        assert!(report.branch_proposals[0].matched_tool.is_none());
        assert!(report
            .apply_failures
            .iter()
            .any(|failure| failure.reason.contains("研究空白")));
        let point = report
            .raised_decisions
            .iter()
            .find(|point| point.kind == DecisionKind::FundamentalGap)
            .unwrap();
        assert!(point.digest.contains("未找到可访问公开数据源"));
        assert!(point.digest.contains("可能是真实研究空白"));
        assert!(point.digest.contains("ICB treatment cohort"));
        assert!(point.digest.contains("SOURCE DISCOVERY TRACE"));
        assert!(point.digest.contains("cBioPortal"));
        assert!(point.digest.contains("no ICB response labels"));
        assert!(point.digest.contains("需前瞻 ICB 队列"));
        assert!(point.digest.contains("判决保持 inconclusive"));
        assert_eq!(point.recommendation, 0);
        assert_eq!(report.source_discoveries.len(), 1);
        assert_eq!(report.source_discoveries[0].hypothesis_id, hypothesis_id);
        assert!(report.source_discoveries[0]
            .trace
            .contains("QUESTION DATA REQUIREMENTS"));
        assert!(report.source_discoveries[0]
            .trace
            .contains("related-but-insufficient"));
        assert_eq!(event_count(&store, "agent.source_discovery"), 1);
        let payload: String = store
            .connection()
            .query_row(
                "SELECT payload_json FROM events WHERE event_type = ?1",
                params!["agent.source_discovery"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(payload.contains("ICB treatment cohort"));
        assert!(payload.contains("related-but-insufficient"));
        assert!(report.applied.is_empty());
        assert_eq!(event_count(&store, "tool_registered"), 0);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn pending_source_discovery_gap_only_counts_fundamental_gap_points() {
        let (path, store) = init_project("source-gap-kind-filter");
        let hypothesis_id = "hyp_source_gap";

        assert!(!store
            .has_pending_source_discovery_gap(hypothesis_id)
            .unwrap());

        let deepen_digest = super::auto_synth_research_gap_digest(
            hypothesis_id,
            "MID1IP1 immunotherapy response needs public ICB data",
            "fixture deepen marker",
            None,
        );
        store
            .raise_decision_point(
                DecisionKind::DeepenOrStop,
                &deepen_digest,
                super::auto_synth_research_gap_options(hypothesis_id),
                0,
            )
            .unwrap();
        assert!(!store
            .has_pending_source_discovery_gap(hypothesis_id)
            .unwrap());

        let gap_digest = super::auto_synth_research_gap_digest(
            hypothesis_id,
            "MID1IP1 immunotherapy response needs public ICB data",
            "fixture fundamental marker",
            None,
        );
        store
            .raise_decision_point(
                DecisionKind::FundamentalGap,
                &gap_digest,
                super::auto_synth_research_gap_options(hypothesis_id),
                0,
            )
            .unwrap();
        assert!(store
            .has_pending_source_discovery_gap(hypothesis_id)
            .unwrap());

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_treats_low_fit_as_gap_but_legacy_propose_synth_does_not() {
        let (path, store) = setup_semantic_project("auto-synth-low-fit-gap");
        let legacy = store
            .run_cycle_with_apply_config(ApplyConfig {
                apply: false,
                auto_run: false,
                flow: None,
                max_apply: 5,
                propose_synth: true,
            })
            .unwrap();
        assert_eq!(
            legacy.branch_proposals[0].matched_tool.as_deref(),
            Some("analysis/score_low_current")
        );
        assert_eq!(
            legacy.branch_proposals[0].matched_fit.as_deref(),
            Some("low")
        );
        assert!(legacy.raised_decisions.is_empty());

        let synthesizer = StubToolSynthesizer::none(&store);
        let auto_synth = store
            .run_cycle_with_synth(
                ApplyConfig {
                    apply: true,
                    auto_run: true,
                    flow: None,
                    max_apply: 5,
                    propose_synth: false,
                },
                &NoopParamInferer,
                &NoopRelevanceScorer,
                &synthesizer,
            )
            .unwrap();

        assert_eq!(synthesizer.calls().len(), 1);
        assert!(synthesizer.calls()[0].1.contains("fit=low"));
        assert_eq!(
            auto_synth.branch_proposals[0].matched_tool.as_deref(),
            Some("analysis/score_low_current")
        );
        assert!(auto_synth.branch_proposals[0].drafted_step.is_none());
        assert!(auto_synth
            .apply_failures
            .iter()
            .any(|failure| failure.reason.contains("auto-synth skipped")));
        assert!(!auto_synth
            .applied
            .iter()
            .any(|action| matches!(action, AppliedAction::StepRun { .. })));
        assert_eq!(event_count(&store, "agent.tool_synthesized"), 0);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_budget_preflight_does_not_register_tool() {
        let (path, store) = init_project("auto-synth-budget-preflight");
        let hypothesis_id =
            record_hypothesis(&store, "Auto synth budget gate needs custom validation");
        link_evidence(
            &store,
            &hypothesis_id,
            EvidenceGrade::LiteratureSupported,
            Stance::Supports,
            "Literature support alone is provisional.",
        );
        let synthesizer = StubToolSynthesizer::registering(&store, &path, "auto_synth_budget");

        let report = store
            .run_cycle_with_synth(
                ApplyConfig {
                    apply: false,
                    auto_run: false,
                    flow: None,
                    max_apply: 0,
                    propose_synth: false,
                },
                &NoopParamInferer,
                &NoopRelevanceScorer,
                &synthesizer,
            )
            .unwrap();

        assert!(synthesizer.calls().is_empty());
        assert_eq!(event_count(&store, "tool_registered"), 0);
        assert_eq!(event_count(&store, "agent.tool_synthesized"), 0);
        assert!(report.applied.is_empty());
        assert!(report.apply_failures.is_empty());
        assert!(report
            .raised_decisions
            .iter()
            .any(|point| point.kind == DecisionKind::BudgetThreshold));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_default_off_preserves_low_fit_application_behavior() {
        let (path, store) = setup_semantic_project("auto-synth-default-off-low-fit");

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
            report.branch_proposals[0].matched_tool.as_deref(),
            Some("analysis/score_low_current")
        );
        assert_eq!(
            report.branch_proposals[0].matched_fit.as_deref(),
            Some("low")
        );
        assert!(report.branch_proposals[0].drafted_step.is_some());
        assert!(report
            .apply_failures
            .iter()
            .any(|failure| failure.reason.contains("artifact_REPLACE_cohort_table")));
        assert_eq!(event_count(&store, "agent.tool_synthesized"), 0);

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
    fn semantic_relevance_true_promotes_low_candidate_and_changes_top_choice() {
        let (path, store) = setup_semantic_project("semantic-true-rerank");
        let scorer = StubRelevanceScorer::relevant("analysis/latent_assoc");

        let report = store
            .run_cycle_with_scorer(ApplyConfig::default(), &NoopParamInferer, &scorer)
            .unwrap();

        let proposal = &report.branch_proposals[0];
        assert_eq!(
            proposal.matched_tool.as_deref(),
            Some("analysis/latent_assoc")
        );
        assert_eq!(proposal.matched_fit.as_deref(), Some("medium"));
        assert!(proposal
            .match_reason
            .as_deref()
            .unwrap()
            .contains("relevance:semantic"));
        assert_eq!(
            scorer.calls(),
            vec![
                "analysis/score_low_current".to_string(),
                "analysis/latent_assoc".to_string()
            ]
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn semantic_relevance_false_or_none_do_not_promote_or_rerank() {
        for (suffix, result) in [("false", Some(false)), ("none", None)] {
            let (path, store) = setup_semantic_project(&format!("semantic-{suffix}-no-promote"));
            let scorer = StubRelevanceScorer::always(result);

            let report = store
                .run_cycle_with_scorer(ApplyConfig::default(), &NoopParamInferer, &scorer)
                .unwrap();

            let proposal = &report.branch_proposals[0];
            assert_eq!(
                proposal.matched_tool.as_deref(),
                Some("analysis/score_low_current")
            );
            assert_eq!(proposal.matched_fit.as_deref(), Some("low"));
            assert!(!proposal
                .match_reason
                .as_deref()
                .unwrap()
                .contains("relevance:semantic"));

            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[test]
    fn run_cycle_with_uses_noop_relevance_scorer_by_default() {
        let (path, store) = setup_semantic_project("semantic-noop-default");

        let report = store
            .run_cycle_with(ApplyConfig::default(), &NoopParamInferer)
            .unwrap();

        let proposal = &report.branch_proposals[0];
        assert_eq!(
            proposal.matched_tool.as_deref(),
            Some("analysis/score_low_current")
        );
        assert_eq!(proposal.matched_fit.as_deref(), Some("low"));
        assert!(!proposal
            .match_reason
            .as_deref()
            .unwrap()
            .contains("relevance:semantic"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn noop_relevance_scorer_preserves_existing_branch_matching() {
        let (path, store) = setup_semantic_project("semantic-noop-explicit");
        let scorer = NoopRelevanceScorer;

        let report = store
            .run_cycle_with_scorer(ApplyConfig::default(), &NoopParamInferer, &scorer)
            .unwrap();

        let proposal = &report.branch_proposals[0];
        assert_eq!(
            proposal.matched_tool.as_deref(),
            Some("analysis/score_low_current")
        );
        assert_eq!(proposal.matched_fit.as_deref(), Some("low"));
        assert!(!proposal
            .match_reason
            .as_deref()
            .unwrap()
            .contains("relevance:semantic"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn semantic_relevance_false_demotes_keyword_medium_candidate_and_reranks() {
        let (path, store) = init_project("semantic-keyword-demote-direct");
        register_semantic_tool(
            &store,
            "thrsp_survival_proxy",
            "verified",
            "THRSP survival association proxy for related cohort analysis",
            &[],
        );
        register_semantic_tool(
            &store,
            "fallback_low",
            "verified",
            "fallback validation helper",
            &[],
        );
        let scorer = StubRelevanceScorer::always(Some(false));
        let mut candidates = vec![
            ToolCandidate {
                tool_ref: "analysis/thrsp_survival_proxy".to_string(),
                fit: Fit::Medium,
                score: 8,
                reason: "keyword:name:thrsp, relevance:keyword".to_string(),
            },
            ToolCandidate {
                tool_ref: "analysis/fallback_low".to_string(),
                fit: Fit::Low,
                score: 20,
                reason: "maturity:verified".to_string(),
            },
        ];

        let changed = super::apply_semantic_relevance_to_candidates(
            &store,
            &mut candidates,
            "THRSP survival mechanism needs validation",
            &scorer,
        )
        .unwrap();

        assert!(changed);
        assert_eq!(
            scorer.calls(),
            vec![
                "analysis/thrsp_survival_proxy".to_string(),
                "analysis/fallback_low".to_string()
            ]
        );
        assert_eq!(candidates[0].tool_ref, "analysis/fallback_low");
        assert_eq!(candidates[0].fit, Fit::Low);
        assert_eq!(candidates[1].tool_ref, "analysis/thrsp_survival_proxy");
        assert_eq!(candidates[1].fit, Fit::Low);
        assert!(candidates[1]
            .reason
            .contains("relevance:demoted_question_mismatch"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn keyword_medium_demoted_to_low_triggers_auto_synth_gap() {
        let (path, store) = setup_keyword_relevance_project("semantic-keyword-demote-auto-synth");
        let scorer = StubRelevanceScorer::always(Some(false));
        let synthesizer = StubToolSynthesizer::none(&store);

        let report = store
            .run_cycle_with_synth(
                ApplyConfig {
                    apply: false,
                    auto_run: false,
                    flow: None,
                    max_apply: 5,
                    propose_synth: false,
                },
                &NoopParamInferer,
                &scorer,
                &synthesizer,
            )
            .unwrap();

        let proposal = &report.branch_proposals[0];
        assert_eq!(
            proposal.matched_tool.as_deref(),
            Some("analysis/thrsp_survival_proxy")
        );
        assert_eq!(proposal.matched_fit.as_deref(), Some("low"));
        assert!(proposal
            .match_reason
            .as_deref()
            .unwrap()
            .contains("relevance:demoted_question_mismatch"));
        assert_eq!(
            scorer.calls(),
            vec!["analysis/thrsp_survival_proxy".to_string()]
        );
        assert_eq!(synthesizer.calls().len(), 1);
        assert!(synthesizer.calls()[0].1.contains("fit=low"));
        assert!(report
            .apply_failures
            .iter()
            .any(|failure| failure.reason.contains("auto-synth skipped")));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn run_cycle_caches_semantic_relevance_scoring_per_tool_and_hypothesis() {
        let (path, store) = setup_keyword_relevance_project("semantic-cycle-cache");
        let scorer = StubRelevanceScorer::always(Some(true));

        let report = store
            .run_cycle_with_scorer(
                ApplyConfig {
                    apply: true,
                    auto_run: false,
                    flow: None,
                    max_apply: 5,
                    propose_synth: false,
                },
                &NoopParamInferer,
                &scorer,
            )
            .unwrap();

        assert_eq!(
            report.branch_proposals[0].matched_tool.as_deref(),
            Some("analysis/thrsp_survival_proxy")
        );
        assert_eq!(
            scorer.calls(),
            vec!["analysis/thrsp_survival_proxy".to_string()]
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn question_mismatch_demotion_prevents_equivalent_branch_brake_and_allows_auto_synth() {
        let (path, store) =
            setup_equivalent_keyword_relevance_project("semantic-equivalent-demote-auto-synth");
        let scorer = StubRelevanceScorer::always(Some(false));
        let synthesizer = StubToolSynthesizer::none(&store);
        let decision = selected_branch_decision(&store);

        assert!(!store
            .has_equivalent_tool_branches(&decision, &[], &scorer)
            .unwrap());

        let report = store
            .run_cycle_with_synth(
                ApplyConfig {
                    apply: false,
                    auto_run: false,
                    flow: None,
                    max_apply: 5,
                    propose_synth: false,
                },
                &NoopParamInferer,
                &scorer,
                &synthesizer,
            )
            .unwrap();

        let proposal = &report.branch_proposals[0];
        assert_eq!(
            proposal.matched_tool.as_deref(),
            Some("analysis/thrsp_survival_proxy")
        );
        assert_eq!(proposal.matched_fit.as_deref(), Some("low"));
        assert!(proposal
            .match_reason
            .as_deref()
            .unwrap()
            .contains("relevance:demoted_question_mismatch"));
        assert_eq!(synthesizer.calls().len(), 1);
        assert!(report
            .apply_failures
            .iter()
            .any(|failure| failure.reason.contains("auto-synth skipped")));
        assert!(!report
            .raised_decisions
            .iter()
            .any(|point| point.kind == DecisionKind::DeepenOrStop));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn question_confirmed_equivalent_branches_still_trigger_deepen_or_stop() {
        let (path, store) =
            setup_equivalent_keyword_relevance_project("semantic-equivalent-true-brake");
        let scorer = StubRelevanceScorer::always(Some(true));
        let synthesizer = StubToolSynthesizer::none(&store);
        let decision = selected_branch_decision(&store);

        assert!(store
            .has_equivalent_tool_branches(&decision, &[], &scorer)
            .unwrap());

        let report = store
            .run_cycle_with_synth(
                ApplyConfig {
                    apply: true,
                    auto_run: false,
                    flow: None,
                    max_apply: 5,
                    propose_synth: false,
                },
                &NoopParamInferer,
                &scorer,
                &synthesizer,
            )
            .unwrap();

        let proposal = &report.branch_proposals[0];
        assert_eq!(
            proposal.matched_tool.as_deref(),
            Some("analysis/thrsp_survival_proxy")
        );
        assert_eq!(proposal.matched_fit.as_deref(), Some("medium"));
        assert!(proposal
            .match_reason
            .as_deref()
            .unwrap()
            .contains("relevance:keyword"));
        assert!(report
            .raised_decisions
            .iter()
            .any(|point| point.kind == DecisionKind::DeepenOrStop));
        assert!(!report.applied.iter().any(|action| matches!(
            action,
            AppliedAction::GraphPatchApplied { .. }
                | AppliedAction::FlowAutoCreated { .. }
                | AppliedAction::StepRun { .. }
        )));
        assert!(synthesizer.calls().is_empty());

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn single_question_relevant_keyword_candidate_does_not_trigger_equivalent_branch_brake() {
        let (path, store) = setup_keyword_relevance_project("semantic-single-keyword-no-brake");
        let scorer = StubRelevanceScorer::always(Some(true));
        let synthesizer = StubToolSynthesizer::none(&store);
        let decision = selected_branch_decision(&store);

        assert!(!store
            .has_equivalent_tool_branches(&decision, &[], &scorer)
            .unwrap());

        let report = store
            .run_cycle_with_synth(
                ApplyConfig {
                    apply: true,
                    auto_run: false,
                    flow: None,
                    max_apply: 5,
                    propose_synth: false,
                },
                &NoopParamInferer,
                &scorer,
                &synthesizer,
            )
            .unwrap();

        let proposal = &report.branch_proposals[0];
        assert_eq!(
            proposal.matched_tool.as_deref(),
            Some("analysis/thrsp_survival_proxy")
        );
        assert_eq!(proposal.matched_fit.as_deref(), Some("medium"));
        assert!(!report
            .raised_decisions
            .iter()
            .any(|point| point.kind == DecisionKind::DeepenOrStop));
        assert!(synthesizer.calls().is_empty());

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn keyword_medium_confirmed_relevant_preserves_existing_auto_synth_behavior() {
        let (path, store) = setup_keyword_relevance_project("semantic-keyword-true-preserve");
        let scorer = StubRelevanceScorer::always(Some(true));
        let synthesizer = StubToolSynthesizer::none(&store);

        let report = store
            .run_cycle_with_synth(
                ApplyConfig {
                    apply: false,
                    auto_run: false,
                    flow: None,
                    max_apply: 5,
                    propose_synth: false,
                },
                &NoopParamInferer,
                &scorer,
                &synthesizer,
            )
            .unwrap();

        let proposal = &report.branch_proposals[0];
        assert_eq!(
            proposal.matched_tool.as_deref(),
            Some("analysis/thrsp_survival_proxy")
        );
        assert_eq!(proposal.matched_fit.as_deref(), Some("medium"));
        assert!(proposal
            .match_reason
            .as_deref()
            .unwrap()
            .contains("relevance:keyword"));
        assert_eq!(
            scorer.calls(),
            vec!["analysis/thrsp_survival_proxy".to_string()]
        );
        assert!(synthesizer.calls().is_empty());
        assert!(report.apply_failures.is_empty());

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn semantic_relevance_does_not_trigger_for_high_fit_candidates() {
        let (path, store) = init_project("semantic-high-skip");
        let scorer = StubRelevanceScorer::always(Some(true));
        let mut candidates = vec![ToolCandidate {
            tool_ref: "analysis/unregistered_high".to_string(),
            fit: Fit::High,
            score: 1,
            reason: "io:exact".to_string(),
        }];

        let promoted = super::apply_semantic_relevance_to_candidates(
            &store,
            &mut candidates,
            "THRSP survival mechanism needs validation",
            &scorer,
        )
        .unwrap();

        assert!(!promoted);
        assert_eq!(candidates[0].fit, Fit::High);
        assert!(scorer.calls().is_empty());

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn semantic_relevance_calls_are_capped_to_top_three_low_candidates() {
        let (path, store) = init_project("semantic-top-k");
        for name in ["low_one", "low_two", "low_three", "outside_target"] {
            register_semantic_tool(
                &store,
                name,
                "verified",
                "validation helper for bounded semantic scoring",
                &[],
            );
        }
        let scorer = StubRelevanceScorer::relevant("analysis/outside_target");
        let mut candidates = vec![
            ToolCandidate {
                tool_ref: "analysis/low_one".to_string(),
                fit: Fit::Low,
                score: 8,
                reason: "candidate one".to_string(),
            },
            ToolCandidate {
                tool_ref: "analysis/low_two".to_string(),
                fit: Fit::Low,
                score: 7,
                reason: "candidate two".to_string(),
            },
            ToolCandidate {
                tool_ref: "analysis/low_three".to_string(),
                fit: Fit::Low,
                score: 6,
                reason: "candidate three".to_string(),
            },
            ToolCandidate {
                tool_ref: "analysis/outside_target".to_string(),
                fit: Fit::Low,
                score: 5,
                reason: "candidate four".to_string(),
            },
        ];

        let promoted = super::apply_semantic_relevance_to_candidates(
            &store,
            &mut candidates,
            "THRSP survival mechanism needs validation",
            &scorer,
        )
        .unwrap();

        assert!(!promoted);
        assert_eq!(
            scorer.calls(),
            vec![
                "analysis/low_one".to_string(),
                "analysis/low_two".to_string(),
                "analysis/low_three".to_string()
            ]
        );
        assert_eq!(candidates[3].fit, Fit::Low);

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
