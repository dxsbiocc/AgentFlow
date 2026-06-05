use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use agentflow_core::agent::{
    AppliedAction, ApplyConfig, CycleReport, EnrichedProposal, NoopParamInferer,
    NoopRelevanceScorer, ParamInferer, RelevanceScorer, ToolSynthesisOutcome, ToolSynthesizer,
};
use agentflow_core::argument::{EvidenceLink, Stance, VerdictSummary, VerdictTag};
use agentflow_core::branch::{
    BranchAction, BranchCandidate, BranchDecision, BranchPolicy, CandidateKind, RuleBasedSelector,
    SelectionMode,
};
use agentflow_core::forage::{AccessStatus, ForageObservation};
use agentflow_core::handoff::{DecisionPoint, DecisionStatus};
use agentflow_core::storage::ProjectStore;
use agentflow_core::trace_guard::{Checkpoint, DriftReport, RevertRecord};

use crate::cli_args::*;
use crate::{
    last_value, parse_u32_value, parse_usize_value, project_path_from_json, synth_commands,
    CliError,
};

#[derive(Debug, Default)]
struct PathJsonOptions {
    path: Option<PathBuf>,
    json: bool,
}

#[derive(Debug)]
struct AgentRunOptions {
    project: PathJsonOptions,
    apply: bool,
    auto_run: bool,
    flow: Option<String>,
    max_apply: u32,
    propose_synth: bool,
    auto_synth: bool,
    infer_params: bool,
    semantic_match: bool,
    synthesizer: Option<String>,
    auto_forage: bool,
    forage_max: u32,
    forage_script: Option<PathBuf>,
    python: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct AutoForageSummary {
    hypotheses_foraged: usize,
    observations_linked: usize,
    skipped: Vec<String>,
}

impl Default for AgentRunOptions {
    fn default() -> Self {
        Self {
            project: PathJsonOptions::default(),
            apply: false,
            auto_run: false,
            flow: None,
            max_apply: 5,
            propose_synth: false,
            auto_synth: false,
            infer_params: false,
            semantic_match: false,
            synthesizer: None,
            auto_forage: false,
            forage_max: DEFAULT_AUTO_FORAGE_MAX,
            forage_script: None,
            python: None,
        }
    }
}

#[derive(Debug, Default)]
struct BranchSelectOptions {
    project: PathJsonOptions,
    explore: bool,
}

#[derive(Debug, Default)]
struct DecisionResolveOptions {
    project: PathJsonOptions,
    decision_id: Option<String>,
    choose: Option<usize>,
    note: Option<String>,
}

#[derive(Debug, Default)]
struct ForageObserveOptions {
    project: PathJsonOptions,
    source: Option<String>,
    external_id: Option<String>,
    title: Option<String>,
    access: Option<AccessStatus>,
}

#[derive(Debug, Default)]
struct ForageIngestOptions {
    project: PathJsonOptions,
    source: Option<String>,
    hits_file: Option<PathBuf>,
}

#[derive(Debug, Default)]
struct ForageFetchOptions {
    project: PathJsonOptions,
    query: Option<String>,
    source: Option<String>,
    script: Option<PathBuf>,
    max: Option<u32>,
    python: Option<String>,
}

#[derive(Debug, Default)]
struct ForageLinkOptions {
    project: PathJsonOptions,
    hypothesis_id: Option<String>,
    observation_id: Option<String>,
    stance: Option<Stance>,
    note: Option<String>,
}

#[derive(Debug, Default)]
struct TraceCheckpointOptions {
    project: PathJsonOptions,
    label: Option<String>,
}

const DEFAULT_FORAGE_SOURCE: &str = "pubmed";
const DEFAULT_PUBMED_SCRIPT: &str = "examples/tools/pubmed_search.py";
const DEFAULT_PUBMED_MAX: u32 = 10;
const DEFAULT_AUTO_FORAGE_MAX: u32 = 5;
const DEFAULT_PYTHON: &str = "python3";
const AUTO_FORAGE_NOTE: &str = "auto-forage";

pub(crate) fn agent_command(args: AgentArgs) -> Result<String, CliError> {
    match args.command {
        AgentCommand::Run(args) => agent_run_command(args),
    }
}

pub(crate) fn branch_command(args: BranchArgs) -> Result<String, CliError> {
    match args.command {
        BranchCommand::Candidates(args) => branch_candidates_command(args),
        BranchCommand::Select(args) => branch_select_command(args),
    }
}

pub(crate) fn decision_command(args: DecisionArgs) -> Result<String, CliError> {
    match args.command {
        DecisionCommand::List(args) => decision_list_command(args),
        DecisionCommand::Pending(args) => decision_pending_command(args),
        DecisionCommand::Show(args) => decision_show_command(args),
        DecisionCommand::Resolve(args) => decision_resolve_command(args),
    }
}

pub(crate) fn forage_command(args: ForageArgs) -> Result<String, CliError> {
    match args.command {
        ForageCommand::Observe(args) => forage_observe_command(args),
        ForageCommand::Ingest(args) => forage_ingest_command(args),
        ForageCommand::Fetch(args) => forage_fetch_command(args),
        ForageCommand::List(args) => forage_list_command(args),
        ForageCommand::Show(args) => forage_show_command(args),
        ForageCommand::Link(args) => forage_link_command(args),
    }
}

pub(crate) fn trace_command(args: TraceArgs) -> Result<String, CliError> {
    match args.command {
        TraceCommand::Checkpoint(args) => trace_checkpoint_command(args),
        TraceCommand::List(args) => trace_list_command(args),
        TraceCommand::Drift(args) => trace_drift_command(args),
        TraceCommand::Revert(args) => trace_revert_command(args),
    }
}

fn agent_run_command(args: AgentRunArgs) -> Result<String, CliError> {
    let options = AgentRunOptions::try_from(args)?;
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = ProjectStore::open(&path)?;
    let auto_forage = if options.auto_forage {
        let script = options
            .forage_script
            .unwrap_or_else(|| PathBuf::from(DEFAULT_PUBMED_SCRIPT));
        validate_forage_script(&script, "agent run --auto-forage", "--forage-script")?;
        let python = options.python.unwrap_or_else(|| DEFAULT_PYTHON.to_string());
        validate_python(&python, "agent run --auto-forage")?;
        Some(auto_forage_pass(
            &store,
            &python,
            &script,
            options.forage_max,
        )?)
    } else {
        None
    };
    let config = ApplyConfig {
        apply: options.apply,
        auto_run: options.auto_run,
        flow: options.flow,
        max_apply: options.max_apply,
        propose_synth: options.propose_synth,
    };
    let synthesizer =
        synth_commands::configured_or_default_synthesizer(store.root_path(), options.synthesizer)?;
    let report = if options.auto_synth {
        let inferer = LlmParamInferer {
            store: &store,
            synthesizer: &synthesizer,
        };
        let scorer = LlmRelevanceScorer {
            store: &store,
            synthesizer: &synthesizer,
        };
        let tool_synthesizer = LlmToolSynthesizer {
            store: &store,
            synthesizer: &synthesizer,
        };
        let noop_inferer = NoopParamInferer;
        let noop_scorer = NoopRelevanceScorer;
        let inferer: &dyn ParamInferer = if options.infer_params {
            &inferer
        } else {
            &noop_inferer
        };
        let scorer: &dyn RelevanceScorer = if options.semantic_match {
            &scorer
        } else {
            &noop_scorer
        };
        store.run_cycle_with_synth(config, inferer, scorer, &tool_synthesizer)?
    } else {
        match (options.infer_params, options.semantic_match) {
            (true, true) => {
                let inferer = LlmParamInferer {
                    store: &store,
                    synthesizer: &synthesizer,
                };
                let scorer = LlmRelevanceScorer {
                    store: &store,
                    synthesizer: &synthesizer,
                };
                store.run_cycle_with_scorer(config, &inferer, &scorer)?
            }
            (true, false) => {
                let inferer = LlmParamInferer {
                    store: &store,
                    synthesizer: &synthesizer,
                };
                store.run_cycle_with(config, &inferer)?
            }
            (false, true) => {
                let scorer = LlmRelevanceScorer {
                    store: &store,
                    synthesizer: &synthesizer,
                };
                store.run_cycle_with_scorer(config, &NoopParamInferer, &scorer)?
            }
            (false, false) => store.run_cycle_with_apply_config(config)?,
        }
    };

    if options.project.json {
        Ok(format_agent_run_json(&report, auto_forage.as_ref()))
    } else {
        Ok(format_agent_run_human(&report, auto_forage.as_ref()))
    }
}

fn branch_candidates_command(args: PathJsonArgs) -> Result<String, CliError> {
    let options = PathJsonOptions::from(args);
    let path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let candidates = store.branch_candidates()?;

    if options.json {
        Ok(branch_candidates_json(&candidates))
    } else if candidates.is_empty() {
        Ok("No branch candidates available".to_string())
    } else {
        Ok(candidates
            .iter()
            .map(format_branch_candidate)
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn branch_select_command(args: BranchSelectArgs) -> Result<String, CliError> {
    let options = BranchSelectOptions::from(args);
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let selector = RuleBasedSelector;
    let decisions = store.select_branches(
        &selector,
        &BranchPolicy {
            explore_enabled: options.explore,
        },
    )?;

    if options.project.json {
        Ok(branch_decisions_json(&decisions))
    } else if decisions.is_empty() {
        Ok("No branch decisions available".to_string())
    } else {
        Ok(decisions
            .iter()
            .map(format_branch_decision)
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn decision_list_command(args: PathJsonArgs) -> Result<String, CliError> {
    let options = PathJsonOptions::from(args);
    let path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let points = store.list_decision_points()?;

    if options.json {
        Ok(decision_points_json(
            "agentflow.decision_points.v0",
            &points,
        ))
    } else if points.is_empty() {
        Ok("No decision points recorded".to_string())
    } else {
        Ok(points
            .iter()
            .map(|point| format_decision_point("Decision point", point))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn decision_pending_command(args: PathJsonArgs) -> Result<String, CliError> {
    let options = PathJsonOptions::from(args);
    let path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let points = store.pending_decision_points()?;

    if options.json {
        Ok(decision_points_json(
            "agentflow.pending_decision_points.v0",
            &points,
        ))
    } else if points.is_empty() {
        Ok("No pending decision points".to_string())
    } else {
        Ok(points
            .iter()
            .map(|point| format_decision_point("Pending decision point", point))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn decision_show_command(args: DecisionShowArgs) -> Result<String, CliError> {
    let decision_id = args.decision_id;
    let json = args.project.json;
    let path = project_path_from_json(args.project)?;
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let point = store.inspect_decision_point(&decision_id)?;

    if json {
        Ok(point.to_json())
    } else {
        Ok(format_decision_point("Decision point", &point))
    }
}

fn decision_resolve_command(args: DecisionResolveArgs) -> Result<String, CliError> {
    let options = DecisionResolveOptions::try_from(args)?;
    let decision_id = options.decision_id.expect("clap requires decision id");
    let chosen_index = options.choose.ok_or_else(|| {
        CliError::InvalidArgument("decision resolve requires --choose".to_string())
    })?;
    let note = options
        .note
        .ok_or_else(|| CliError::InvalidArgument("decision resolve requires --note".to_string()))?;
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let point = store.resolve_decision_point(&decision_id, chosen_index, &note)?;

    if options.project.json {
        Ok(point.to_json())
    } else {
        Ok(format_decision_point("Resolved decision point", &point))
    }
}

fn forage_observe_command(args: ForageObserveArgs) -> Result<String, CliError> {
    let options = ForageObserveOptions::try_from(args)?;
    let source = options
        .source
        .ok_or_else(|| CliError::InvalidArgument("forage observe requires --source".to_string()))?;
    let external_id = options.external_id.ok_or_else(|| {
        CliError::InvalidArgument("forage observe requires --external-id".to_string())
    })?;
    let title = options
        .title
        .ok_or_else(|| CliError::InvalidArgument("forage observe requires --title".to_string()))?;
    let access = options
        .access
        .ok_or_else(|| CliError::InvalidArgument("forage observe requires --access".to_string()))?;
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let observation = store.record_forage_observation(&source, &external_id, &title, access)?;

    if options.project.json {
        Ok(observation.to_json())
    } else {
        Ok(format_forage_observation(
            "Recorded forage observation",
            &observation,
        ))
    }
}

fn forage_ingest_command(args: ForageIngestArgs) -> Result<String, CliError> {
    let options = ForageIngestOptions::from(args);
    let hits_file = options.hits_file.expect("clap requires hits file");
    let source = options
        .source
        .unwrap_or_else(|| DEFAULT_FORAGE_SOURCE.to_string());
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let observations = ingest_forage_hits(&store, &hits_file, &source)?;

    if options.project.json {
        Ok(forage_ingest_summary_json(&observations))
    } else {
        Ok(format_forage_ingest_summary(&observations))
    }
}

fn forage_fetch_command(args: ForageFetchArgs) -> Result<String, CliError> {
    let options = ForageFetchOptions::try_from(args)?;
    let query = options
        .query
        .ok_or_else(|| CliError::InvalidArgument("forage fetch requires --query".to_string()))?;
    if query.trim().is_empty() {
        return Err(CliError::InvalidArgument(
            "forage fetch requires --query".to_string(),
        ));
    }

    let source = options
        .source
        .unwrap_or_else(|| DEFAULT_FORAGE_SOURCE.to_string());
    let script = options
        .script
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PUBMED_SCRIPT));
    validate_forage_script(&script, "forage fetch", "--script")?;

    let python = options.python.unwrap_or_else(|| DEFAULT_PYTHON.to_string());
    validate_python(&python, "forage fetch")?;

    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = ProjectStore::open(&path)?;
    let max = options.max.unwrap_or(DEFAULT_PUBMED_MAX);
    let observations = fetch_and_ingest(&store, &python, &script, &query, max, &source)?;

    if options.project.json {
        Ok(forage_ingest_summary_json(&observations))
    } else {
        Ok(format_forage_ingest_summary(&observations))
    }
}

fn fetch_and_ingest(
    store: &ProjectStore,
    python: &str,
    script: &Path,
    query: &str,
    max: u32,
    source: &str,
) -> Result<Vec<ForageObservation>, CliError> {
    let out_file = forage_fetch_tmp_path();
    let output = Command::new(python)
        .arg(script)
        .arg("--query")
        .arg(query)
        .arg("--max")
        .arg(max.to_string())
        .arg("--out")
        .arg(&out_file)
        .output()
        .map_err(|error| {
            let _ = std::fs::remove_file(&out_file);
            CliError::Core(format!(
                "failed to run forage fetch script {} with {}: {error}",
                script.display(),
                python
            ))
        })?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&out_file);
        return Err(CliError::Core(format!(
            "forage fetch script failed with status {}: {}",
            format_exit_status(&output.status),
            stderr_summary(&output.stderr)
        )));
    }

    let observations = ingest_forage_hits(store, &out_file, source);
    let _ = std::fs::remove_file(&out_file);
    observations
}

fn auto_forage_pass(
    store: &ProjectStore,
    python: &str,
    script: &Path,
    max: u32,
) -> Result<AutoForageSummary, CliError> {
    let mut summary = AutoForageSummary::default();

    for hypothesis in store.list_hypotheses()? {
        let verdict = store.latest_verdict_for(&hypothesis.id)?;
        if !should_auto_forage(verdict.as_ref()) {
            continue;
        }

        let observations = match fetch_and_ingest(
            store,
            python,
            script,
            &hypothesis.statement,
            max,
            DEFAULT_FORAGE_SOURCE,
        ) {
            Ok(observations) => observations,
            Err(error) => {
                summary
                    .skipped
                    .push(format!("{}: {}", hypothesis.id, error.message()));
                continue;
            }
        };

        summary.hypotheses_foraged += 1;
        for observation in observations {
            store.link_forage_evidence(
                &hypothesis.id,
                &observation.id,
                Stance::Neutral,
                AUTO_FORAGE_NOTE,
            )?;
            summary.observations_linked += 1;
        }
    }

    Ok(summary)
}

struct LlmParamInferer<'a> {
    store: &'a ProjectStore,
    synthesizer: &'a str,
}

impl ParamInferer for LlmParamInferer<'_> {
    fn infer(&self, hypothesis_statement: &str, param_name: &str) -> Option<String> {
        let prompt = param_inference_prompt(hypothesis_statement, param_name);
        let candidate = synth_commands::run_project_synthesizer(
            self.store.root_path(),
            self.synthesizer,
            &prompt,
        )
        .ok()?;
        let stripped = synth_commands::strip_markdown_fence(&candidate);
        first_non_empty_line(&stripped).map(ToOwned::to_owned)
    }
}

struct LlmRelevanceScorer<'a> {
    store: &'a ProjectStore,
    synthesizer: &'a str,
}

impl RelevanceScorer for LlmRelevanceScorer<'_> {
    fn is_relevant(
        &self,
        hypothesis_statement: &str,
        tool_ref: &str,
        tool_description: &str,
    ) -> Option<bool> {
        let prompt = relevance_prompt(hypothesis_statement, tool_ref, tool_description);
        let candidate = synth_commands::run_project_synthesizer(
            self.store.root_path(),
            self.synthesizer,
            &prompt,
        )
        .ok()?;
        let stripped = synth_commands::strip_markdown_fence(&candidate);
        parse_yes_no(&stripped)
    }
}

struct LlmToolSynthesizer<'a> {
    store: &'a ProjectStore,
    synthesizer: &'a str,
}

impl ToolSynthesizer for LlmToolSynthesizer<'_> {
    fn synthesize(
        &self,
        hypothesis_statement: &str,
        capability_need: &str,
    ) -> ToolSynthesisOutcome {
        match synth_commands::auto_synthesize_agent_tool(
            self.store,
            self.synthesizer,
            hypothesis_statement,
            capability_need,
        ) {
            Ok(synth_commands::AutoSynthToolResult::Registered(tool_ref)) => {
                ToolSynthesisOutcome::registered(tool_ref)
            }
            Ok(synth_commands::AutoSynthToolResult::Rejected(reason)) => {
                ToolSynthesisOutcome::rejected(reason)
            }
            Err(error) => ToolSynthesisOutcome::rejected(format!(
                "auto-synth backend or registration failed: {}",
                error.message()
            )),
        }
    }
}

fn param_inference_prompt(statement: &str, param_name: &str) -> String {
    format!(
        "Research hypothesis: \"{statement}\". A bioinformatics analysis tool needs a value for the parameter \"{param_name}\". Reply with ONLY the value (e.g. a gene symbol like THRSP), no explanation, no quotes."
    )
}

fn relevance_prompt(statement: &str, tool_ref: &str, tool_description: &str) -> String {
    format!("假设「{statement}」与工具 <{tool_ref}>（描述：{tool_description}）是否研究相关？只答 yes/no。")
}

fn first_non_empty_line(value: &str) -> Option<&str> {
    value.lines().map(str::trim).find(|line| !line.is_empty())
}

fn parse_yes_no(value: &str) -> Option<bool> {
    let answer = first_non_empty_line(value)?;
    let normalized = answer
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '.' | '。'))
        .to_ascii_lowercase();
    match normalized.as_str() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

fn should_auto_forage(verdict: Option<&VerdictSummary>) -> bool {
    matches!(
        verdict.map(|summary| summary.tag),
        None | Some(VerdictTag::InconclusiveProvisional)
    )
}

fn forage_list_command(args: PathJsonArgs) -> Result<String, CliError> {
    let options = PathJsonOptions::from(args);
    let path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let observations = store.list_forage_observations()?;

    if options.json {
        Ok(forage_observations_json(&observations))
    } else if observations.is_empty() {
        Ok("No forage observations recorded".to_string())
    } else {
        Ok(observations
            .iter()
            .map(format_forage_observation_summary)
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn forage_show_command(args: ForageShowArgs) -> Result<String, CliError> {
    let observation_id = args.forage_obs_id;
    let json = args.project.json;
    let path = project_path_from_json(args.project)?;
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let observation = store.inspect_forage_observation(&observation_id)?;

    if json {
        Ok(observation.to_json())
    } else {
        Ok(format_forage_observation(
            "Forage observation",
            &observation,
        ))
    }
}

fn forage_link_command(args: ForageLinkArgs) -> Result<String, CliError> {
    let options = ForageLinkOptions::try_from(args)?;
    let hypothesis_id = options.hypothesis_id.ok_or_else(|| {
        CliError::InvalidArgument("forage link requires --hypothesis".to_string())
    })?;
    let observation_id = options.observation_id.ok_or_else(|| {
        CliError::InvalidArgument("forage link requires --observation".to_string())
    })?;
    let stance = options
        .stance
        .ok_or_else(|| CliError::InvalidArgument("forage link requires --stance".to_string()))?;
    let note = options
        .note
        .ok_or_else(|| CliError::InvalidArgument("forage link requires --note".to_string()))?;
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let link = store.link_forage_evidence(&hypothesis_id, &observation_id, stance, &note)?;

    if options.project.json {
        Ok(link.to_json())
    } else {
        Ok(format_evidence_link("Linked forage evidence", &link))
    }
}

fn trace_checkpoint_command(args: TraceCheckpointArgs) -> Result<String, CliError> {
    let options = TraceCheckpointOptions::from(args);
    let label = options.label.ok_or_else(|| {
        CliError::InvalidArgument("trace checkpoint requires --label".to_string())
    })?;
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let checkpoint = store.create_checkpoint(&label)?;

    if options.project.json {
        Ok(checkpoint.to_json())
    } else {
        Ok(format_checkpoint("Created checkpoint", &checkpoint))
    }
}

fn trace_list_command(args: PathJsonArgs) -> Result<String, CliError> {
    let options = PathJsonOptions::from(args);
    let path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let checkpoints = store.list_checkpoints()?;

    if options.json {
        Ok(checkpoints_json(&checkpoints))
    } else if checkpoints.is_empty() {
        Ok("No checkpoints recorded".to_string())
    } else {
        Ok(checkpoints
            .iter()
            .map(format_checkpoint_summary)
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn trace_drift_command(args: TraceCheckpointIdArgs) -> Result<String, CliError> {
    let checkpoint_id = args.checkpoint_id;
    let json = args.project.json;
    let path = project_path_from_json(args.project)?;
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let report = store.detect_drift(&checkpoint_id)?;

    if json {
        Ok(report.to_json())
    } else {
        Ok(format_drift_report(&report))
    }
}

fn trace_revert_command(args: TraceCheckpointIdArgs) -> Result<String, CliError> {
    let checkpoint_id = args.checkpoint_id;
    let json = args.project.json;
    let path = project_path_from_json(args.project)?;
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let record = store.revert_to(&checkpoint_id)?;

    if json {
        Ok(record.to_json())
    } else {
        Ok(format_revert_record(&record))
    }
}

impl From<PathJsonArgs> for PathJsonOptions {
    fn from(args: PathJsonArgs) -> Self {
        Self {
            path: last_value(args.path),
            json: args.json,
        }
    }
}

impl TryFrom<AgentRunArgs> for AgentRunOptions {
    type Error = CliError;

    fn try_from(args: AgentRunArgs) -> Result<Self, Self::Error> {
        let mut options = Self {
            project: PathJsonOptions::from(args.project),
            apply: args.apply,
            auto_run: args.auto_run,
            flow: last_value(args.flow),
            max_apply: 5,
            propose_synth: args.propose_synth,
            auto_synth: args.auto_synth,
            infer_params: args.infer_params,
            semantic_match: args.semantic_match,
            synthesizer: last_value(args.synthesizer),
            auto_forage: args.auto_forage,
            forage_max: DEFAULT_AUTO_FORAGE_MAX,
            forage_script: last_value(args.forage_script),
            python: last_value(args.python),
        };

        if let Some(value) = last_value(args.max_apply) {
            let max_apply = parse_usize_value("--max-apply", &value)?;
            options.max_apply = u32::try_from(max_apply).map_err(|_| {
                CliError::InvalidArgument(
                    "--max-apply must fit in an unsigned 32-bit integer".to_string(),
                )
            })?;
        }
        if let Some(value) = last_value(args.forage_max) {
            options.forage_max = parse_u32_value("--forage-max", &value)?;
        }

        Ok(options)
    }
}

impl From<BranchSelectArgs> for BranchSelectOptions {
    fn from(args: BranchSelectArgs) -> Self {
        Self {
            project: PathJsonOptions::from(args.project),
            explore: args.explore,
        }
    }
}

impl TryFrom<DecisionResolveArgs> for DecisionResolveOptions {
    type Error = CliError;

    fn try_from(args: DecisionResolveArgs) -> Result<Self, Self::Error> {
        Ok(Self {
            project: PathJsonOptions::from(args.project),
            decision_id: Some(args.decision_id),
            choose: last_value(args.choose)
                .map(|value| parse_usize_value("--choose", &value))
                .transpose()?,
            note: last_value(args.note),
        })
    }
}

impl TryFrom<ForageObserveArgs> for ForageObserveOptions {
    type Error = CliError;

    fn try_from(args: ForageObserveArgs) -> Result<Self, Self::Error> {
        Ok(Self {
            project: PathJsonOptions::from(args.project),
            source: last_value(args.source),
            external_id: last_value(args.external_id),
            title: last_value(args.title),
            access: last_value(args.access)
                .map(|access| parse_access_status(&access))
                .transpose()?,
        })
    }
}

impl From<ForageIngestArgs> for ForageIngestOptions {
    fn from(args: ForageIngestArgs) -> Self {
        Self {
            project: PathJsonOptions::from(args.project),
            source: last_value(args.source),
            hits_file: Some(args.hits_file),
        }
    }
}

impl TryFrom<ForageFetchArgs> for ForageFetchOptions {
    type Error = CliError;

    fn try_from(args: ForageFetchArgs) -> Result<Self, Self::Error> {
        Ok(Self {
            project: PathJsonOptions::from(args.project),
            query: last_value(args.query),
            source: last_value(args.source),
            script: last_value(args.script),
            max: last_value(args.max)
                .map(|max| parse_u32_value("--max", &max))
                .transpose()?,
            python: last_value(args.python),
        })
    }
}

impl TryFrom<ForageLinkArgs> for ForageLinkOptions {
    type Error = CliError;

    fn try_from(args: ForageLinkArgs) -> Result<Self, Self::Error> {
        Ok(Self {
            project: PathJsonOptions::from(args.project),
            hypothesis_id: last_value(args.hypothesis),
            observation_id: last_value(args.observation),
            stance: last_value(args.stance)
                .map(|stance| parse_stance(&stance))
                .transpose()?,
            note: last_value(args.note),
        })
    }
}

impl From<TraceCheckpointArgs> for TraceCheckpointOptions {
    fn from(args: TraceCheckpointArgs) -> Self {
        Self {
            project: PathJsonOptions::from(args.project),
            label: last_value(args.label),
        }
    }
}

fn validate_forage_script(script: &Path, command: &str, flag: &str) -> Result<(), CliError> {
    if script.as_os_str().is_empty() {
        return Err(CliError::InvalidArgument(format!(
            "{command} requires {flag}"
        )));
    }
    if !script.exists() {
        return Err(CliError::InvalidArgument(format!(
            "{command} script not found: {}",
            script.display()
        )));
    }
    Ok(())
}

fn validate_python(python: &str, command: &str) -> Result<(), CliError> {
    if python.trim().is_empty() {
        return Err(CliError::InvalidArgument(format!(
            "{command} requires --python"
        )));
    }
    Ok(())
}

fn parse_access_status(value: &str) -> Result<AccessStatus, CliError> {
    AccessStatus::parse(value).ok_or_else(|| {
        CliError::InvalidArgument(
            "--access must be metadata_only, abstract_available, open_access_full_text, user_provided_full_text, subscription_connector_full_text, full_text_unavailable, or retrieval_failed"
                .to_string(),
        )
    })
}

fn parse_stance(value: &str) -> Result<Stance, CliError> {
    Stance::parse(value).ok_or_else(|| {
        CliError::InvalidArgument("--stance must be supports, contradicts, or neutral".to_string())
    })
}

fn branch_candidates_json(candidates: &[BranchCandidate]) -> String {
    let items = candidates
        .iter()
        .map(BranchCandidate::to_json)
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"schema_version\":\"agentflow.branch_candidates.v0\",\"candidates\":[{items}]}}")
}

fn branch_decisions_json(decisions: &[BranchDecision]) -> String {
    let items = decisions
        .iter()
        .map(BranchDecision::to_json)
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"schema_version\":\"agentflow.branch_decisions.v0\",\"decisions\":[{items}]}}")
}

fn decision_points_json(schema_version: &str, points: &[DecisionPoint]) -> String {
    let items = points
        .iter()
        .map(DecisionPoint::to_json)
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"schema_version\":\"{schema_version}\",\"decision_points\":[{items}]}}")
}

fn forage_observations_json(observations: &[ForageObservation]) -> String {
    let items = observations
        .iter()
        .map(ForageObservation::to_json)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"schema_version\":\"agentflow.forage_observations.v0\",\"observations\":[{items}]}}"
    )
}

fn checkpoints_json(checkpoints: &[Checkpoint]) -> String {
    let items = checkpoints
        .iter()
        .map(Checkpoint::to_json)
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"schema_version\":\"agentflow.checkpoints.v0\",\"checkpoints\":[{items}]}}")
}

fn forage_ingest_summary_json(observations: &[ForageObservation]) -> String {
    let ids = observations
        .iter()
        .map(|observation| format!("\"{}\"", escape_json(&observation.id)))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"schema_version\":\"agentflow.forage_ingest.v0\",\"count\":{},\"observation_ids\":[{ids}]}}",
        observations.len()
    )
}

fn format_agent_run_json(report: &CycleReport, auto_forage: Option<&AutoForageSummary>) -> String {
    let report_json = report.to_json();
    let Some(auto_forage) = auto_forage else {
        return report_json;
    };

    let report_without_closing = report_json.strip_suffix('}').unwrap_or(&report_json);
    format!(
        "{report_without_closing},\"auto_forage\":{}}}",
        auto_forage_summary_json(auto_forage)
    )
}

fn auto_forage_summary_json(summary: &AutoForageSummary) -> String {
    let skipped = summary
        .skipped
        .iter()
        .map(|entry| format!("\"{}\"", escape_json(entry)))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        concat!(
            "{{",
            "\"hypotheses_foraged\":{},",
            "\"observations_linked\":{},",
            "\"skipped\":[{}]",
            "}}"
        ),
        summary.hypotheses_foraged, summary.observations_linked, skipped
    )
}

fn format_agent_run_human(report: &CycleReport, auto_forage: Option<&AutoForageSummary>) -> String {
    let report = format_cycle_report(report);
    if let Some(summary) = auto_forage {
        format!("{}\n{report}", format_auto_forage_summary(summary))
    } else {
        report
    }
}

fn format_auto_forage_summary(summary: &AutoForageSummary) -> String {
    format!(
        "Auto-forage\nHypotheses foraged: {}\nObservations linked: {}\nSkipped:\n{}",
        summary.hypotheses_foraged,
        summary.observations_linked,
        format_auto_forage_skipped(&summary.skipped)
    )
}

fn format_auto_forage_skipped(skipped: &[String]) -> String {
    if skipped.is_empty() {
        return "  none".to_string();
    }

    skipped
        .iter()
        .map(|entry| format!("  {entry}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_cycle_report(report: &CycleReport) -> String {
    let base = format!(
        "Agent cycle complete\nCheckpoint: {}\nProvisional verdicts: {}\nStrong candidates: {}\nRaised decisions: {}\nBranch proposals: {}\nOutcome: {}\nDecision points:\n{}\nBranch proposal details:\n{}",
        report.checkpoint_id,
        report.provisional_verdicts.len(),
        report.strong_candidates.len(),
        report.raised_decisions.len(),
        report.branch_proposals.len(),
        report.outcome.as_str(),
        format_cycle_decision_summaries(&report.raised_decisions),
        format_cycle_branch_summaries(&report.branch_proposals)
    );
    if report.applied.is_empty() {
        base
    } else {
        format!(
            "{base}\nApplied:\n{}",
            format_applied_actions(&report.applied)
        )
    }
}

fn format_cycle_decision_summaries(points: &[DecisionPoint]) -> String {
    if points.is_empty() {
        return "  none".to_string();
    }

    points
        .iter()
        .map(|point| format!("  {} [{}] {}", point.id, point.kind.as_str(), point.digest))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_cycle_branch_summaries(proposals: &[EnrichedProposal]) -> String {
    if proposals.is_empty() {
        return "  none".to_string();
    }

    proposals
        .iter()
        .map(|proposal| {
            format!(
                "  {} [{}] {}: {}; {}",
                proposal.decision.candidate.hypothesis_id,
                selection_mode_as_str(proposal.decision.selected_by),
                branch_action_kind(&proposal.decision.action),
                branch_action_reason(&proposal.decision.action),
                branch_match_summary(proposal)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_applied_actions(actions: &[AppliedAction]) -> String {
    actions
        .iter()
        .map(|action| match action {
            AppliedAction::LifecycleTransition { hypothesis_id, to } => {
                format!("  lifecycle {} -> {}", hypothesis_id, to)
            }
            AppliedAction::GraphPatchApplied {
                flow_id,
                patch_id,
                step_id,
            } => format!(
                "  graph patch {} applied to {} step {}",
                patch_id, flow_id, step_id
            ),
            AppliedAction::FlowAutoCreated { flow_id } => {
                format!("  flow {} auto-created", flow_id)
            }
            AppliedAction::StepRun {
                step_id,
                observation_id,
            } => match observation_id {
                Some(observation_id) => {
                    format!("  step {} ran and observed {}", step_id, observation_id)
                }
                None => format!("  step {} ran without observation", step_id),
            },
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn branch_match_summary(proposal: &EnrichedProposal) -> String {
    match proposal.matched_tool.as_deref() {
        Some(tool) => format!(
            "matched tool: {} ({})",
            tool,
            proposal.matched_fit.as_deref().unwrap_or("unknown")
        ),
        None => "no tool match".to_string(),
    }
}

fn format_branch_candidate(candidate: &BranchCandidate) -> String {
    format!(
        "{} [{}/score {}] {}\n  verdict: {}\n  confidence: {}\n  evidence: {}",
        candidate.hypothesis_id,
        candidate_kind_as_str(candidate.kind),
        candidate.score,
        candidate.statement,
        candidate
            .verdict
            .map(|verdict| verdict.as_str())
            .unwrap_or("none"),
        candidate
            .confidence
            .map(|confidence| confidence.as_str())
            .unwrap_or("none"),
        candidate.evidence_count
    )
}

fn format_branch_decision(decision: &BranchDecision) -> String {
    format!(
        "{} [{}] {}: {}\n  candidate: {}\n  score: {}",
        decision.candidate.hypothesis_id,
        selection_mode_as_str(decision.selected_by),
        branch_action_kind(&decision.action),
        branch_action_reason(&decision.action),
        candidate_kind_as_str(decision.candidate.kind),
        decision.candidate.score
    )
}

fn format_decision_point(heading: &str, point: &DecisionPoint) -> String {
    format!(
        "{heading}\nId: {}\nKind: {}\nStatus: {}\nDigest: {}\nRecommendation: {}\nOptions:\n{}\nResolution: {}\nCreated: {}",
        point.id,
        point.kind.as_str(),
        decision_status_as_str(point.status),
        point.digest,
        point.recommendation,
        format_handoff_options(point),
        format_resolution(point),
        point.created_at
    )
}

fn format_handoff_options(point: &DecisionPoint) -> String {
    if point.options.is_empty() {
        return "  none".to_string();
    }
    point
        .options
        .iter()
        .enumerate()
        .map(|(index, option)| {
            format!(
                "  {index}: {} [{} / {} / reversible={}] {}",
                option.label, option.cost, option.risk, option.reversible, option.direction
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_resolution(point: &DecisionPoint) -> String {
    point.resolution.as_ref().map_or_else(
        || "none".to_string(),
        |resolution| {
            format!(
                "chosen {} at {}: {}",
                resolution.chosen_index, resolution.resolved_at, resolution.note
            )
        },
    )
}

fn format_forage_observation(heading: &str, observation: &ForageObservation) -> String {
    format!(
        "{heading}\nId: {}\nSource: {}\nExternal id: {}\nTitle: {}\nAccess: {}\nRetrieved: {}",
        observation.id,
        observation.source_id,
        observation.external_id,
        observation.title,
        observation.access_status,
        observation.retrieved_at
    )
}

fn format_forage_observation_summary(observation: &ForageObservation) -> String {
    format!(
        "{} [{}] {}\n  source: {}\n  external id: {}",
        observation.id,
        observation.access_status,
        observation.title,
        observation.source_id,
        observation.external_id
    )
}

fn format_forage_ingest_summary(observations: &[ForageObservation]) -> String {
    format!(
        "Ingested {} forage observations\nIds:\n{}",
        observations.len(),
        format_forage_ingest_ids(observations)
    )
}

fn format_forage_ingest_ids(observations: &[ForageObservation]) -> String {
    if observations.is_empty() {
        return "  none".to_string();
    }

    observations
        .iter()
        .map(|observation| format!("  {}", observation.id))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_evidence_link(heading: &str, link: &EvidenceLink) -> String {
    format!(
        "{heading}\nId: {}\nHypothesis: {}\nObservation: {}\nSource: {}\nGrade: {}\nStance: {}\nNote: {}\nCreated: {}",
        link.id,
        link.hypothesis_id,
        link.observation_id.as_deref().unwrap_or("none"),
        link.source.as_deref().unwrap_or("none"),
        link.grade,
        link.stance,
        link.note,
        link.created_at
    )
}

fn format_checkpoint(heading: &str, checkpoint: &Checkpoint) -> String {
    format!(
        "{heading}\nId: {}\nHorizon event: {}\nLabel: {}\nCreated: {}",
        checkpoint.id,
        checkpoint.horizon_event_id.as_deref().unwrap_or("none"),
        checkpoint.label,
        checkpoint.created_at
    )
}

fn format_checkpoint_summary(checkpoint: &Checkpoint) -> String {
    format!(
        "{} [{}]\n  horizon: {}",
        checkpoint.id,
        checkpoint.label,
        checkpoint.horizon_event_id.as_deref().unwrap_or("none")
    )
}

fn format_drift_report(report: &DriftReport) -> String {
    format!(
        "Drift report\nCheckpoint: {}\nAutonomous steps: {}\nShould surface: {}\nNet goal delta: {}",
        report.from_checkpoint,
        report.autonomous_steps,
        report.should_surface,
        report.net_goal_delta
    )
}

fn format_revert_record(record: &RevertRecord) -> String {
    format!(
        "Trace revert\nRecord: {}\nCheckpoint: {}\n已记录回退，{} 条事件标记为回退；不物理删除",
        record.id,
        record.checkpoint_id,
        record.reverted_event_ids.len()
    )
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

fn branch_action_kind(action: &BranchAction) -> &'static str {
    match action {
        BranchAction::Deepen { .. } => "deepen",
        BranchAction::Spawn { .. } => "spawn",
        BranchAction::Abandon { .. } => "abandon",
        BranchAction::Hold { .. } => "hold",
    }
}

fn branch_action_reason(action: &BranchAction) -> &str {
    match action {
        BranchAction::Deepen { reason }
        | BranchAction::Spawn { reason }
        | BranchAction::Abandon { reason, .. }
        | BranchAction::Hold { reason } => reason,
    }
}

fn decision_status_as_str(status: DecisionStatus) -> &'static str {
    match status {
        DecisionStatus::Pending => "pending",
        DecisionStatus::Resolved => "resolved",
    }
}

struct ForageHit {
    external_id: String,
    title: String,
    access_status: AccessStatus,
}

fn ingest_forage_hits(
    store: &agentflow_core::storage::ProjectStore,
    hits_file: &Path,
    source: &str,
) -> Result<Vec<ForageObservation>, CliError> {
    let contents = std::fs::read_to_string(hits_file)?;
    let mut observations = Vec::new();

    for (index, line) in contents.lines().enumerate() {
        let line_number = index + 1;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let hit = parse_forage_hit(line, line_number)?;
        observations.push(store.record_forage_observation(
            source,
            &hit.external_id,
            &hit.title,
            hit.access_status,
        )?);
    }

    Ok(observations)
}

fn parse_forage_hit(line: &str, line_number: usize) -> Result<ForageHit, CliError> {
    let external_id = required_jsonl_string(line, "external_id", line_number)?;
    let title = required_jsonl_string(line, "title", line_number)?;
    let access_status_value = required_jsonl_string(line, "access_status", line_number)?;
    let access_status = AccessStatus::parse(&access_status_value).ok_or_else(|| {
        CliError::InvalidArgument(format!(
            "hits JSONL line {line_number} has invalid access_status: {access_status_value}"
        ))
    })?;

    Ok(ForageHit {
        external_id,
        title,
        access_status,
    })
}

fn required_jsonl_string(line: &str, field: &str, line_number: usize) -> Result<String, CliError> {
    json_string_field(line, field).ok_or_else(|| {
        CliError::InvalidArgument(format!("hits JSONL line {line_number} is missing {field}"))
    })
}

fn json_string_field(json: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\"");
    let start = json.find(&marker)? + marker.len();
    let rest = json[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = find_json_string_end(rest)?;
    Some(unescape_json_string(&rest[..end]))
}

fn find_json_string_end(input: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(index),
            _ => {}
        }
    }
    None
}

fn unescape_json_string(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('"') => output.push('"'),
            Some('\\') => output.push('\\'),
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some('t') => output.push('\t'),
            Some('u') => {
                let digits = chars.by_ref().take(4).collect::<String>();
                if let Ok(code) = u32::from_str_radix(&digits, 16) {
                    if let Some(decoded) = char::from_u32(code) {
                        output.push(decoded);
                    }
                }
            }
            Some(other) => output.push(other),
            None => break,
        }
    }
    output
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

fn forage_fetch_tmp_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agentflow-forage-fetch-{}-{nanos}.jsonl",
        std::process::id()
    ))
}

fn format_exit_status(status: &std::process::ExitStatus) -> String {
    status.code().map_or_else(
        || "terminated by signal".to_string(),
        |code| code.to_string(),
    )
}

fn stderr_summary(stderr: &[u8]) -> String {
    let summary = String::from_utf8_lossy(stderr)
        .trim()
        .chars()
        .take(500)
        .collect::<String>();
    if summary.is_empty() {
        "no stderr".to_string()
    } else {
        summary
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use crate::run;
    use agentflow_core::argument::{
        ClaimBasis, EvidenceGrade, EvidenceLinkRequest, RuleBasedEngine, SelfDeceptionGate,
    };
    use agentflow_core::handoff::{Cost, DecisionKind, HandoffOption, Risk};
    use agentflow_core::hypothesis::{HypothesisRequest, HypothesisStatus};
    use agentflow_core::storage::{
        ArtifactImportMode, ArtifactImportRequest, EventRecord, FlowDraft, ProjectStore, ToolSpec,
    };

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-cli-c2-{test_name}-{}-{}",
            std::process::id(),
            agentflow_core::storage::now_unix_seconds()
        ))
    }

    fn init_project(path: &std::path::Path) -> ProjectStore {
        let _ = std::fs::remove_dir_all(path);
        ProjectStore::init(path, Some("C2 Demo")).unwrap()
    }

    fn record_hypothesis(store: &ProjectStore) -> String {
        store
            .record_hypothesis(HypothesisRequest {
                statement: "Marker A supports pathway B".to_string(),
                origin: "c2 test".to_string(),
                related_goal_id: "goal_c2".to_string(),
            })
            .unwrap()
            .id
    }

    fn record_hypothesis_with_statement(store: &ProjectStore, statement: &str) -> String {
        store
            .record_hypothesis(HypothesisRequest {
                statement: statement.to_string(),
                origin: "c2 test".to_string(),
                related_goal_id: "goal_c2".to_string(),
            })
            .unwrap()
            .id
    }

    fn link_weak_evidence(store: &ProjectStore, hypothesis_id: &str) {
        store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: hypothesis_id.to_string(),
                observation_id: None,
                source: None,
                grade: EvidenceGrade::LiteratureSupported,
                stance: Stance::Supports,
                note: "Literature support remains provisional.".to_string(),
            })
            .unwrap();
    }

    fn valid_gate() -> SelfDeceptionGate {
        SelfDeceptionGate {
            supports: "Observed evidence supports the claim".to_string(),
            against: "Contradictory evidence has been checked".to_string(),
            alternatives: "Alternative explanations remain less consistent".to_string(),
            data_quality_risks: "Sampling bias is limited by replication".to_string(),
            assumptions: "Measurements are comparable across runs".to_string(),
            falsifier: "A replicated contradiction would overturn this claim".to_string(),
            claim_basis: ClaimBasis::Observed,
            not_yet_claimable: "No causal mechanism is claimed yet".to_string(),
        }
    }

    fn write_auto_forage_script(path: &Path, fail_query: Option<&str>) {
        let fail_block = fail_query.map_or_else(String::new, |query| {
            format!(
                "if [ \"$query\" = {} ]; then\n  echo \"fixture failure for $query\" >&2\n  exit 7\nfi\n",
                shell_single_quoted(query)
            )
        });
        std::fs::write(
            path,
            format!(
                r#"set -eu
out=""
query=""
max=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --query) query="$2"; shift 2 ;;
    --max) max="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    *) shift ;;
  esac
done
{fail_block}: "${{out:?missing out}}"
printf '{{"external_id":"PMID:390000%s","title":"Auto forage fixture","access_status":"abstract_available"}}\n' "$max" > "$out"
"#
            ),
        )
        .unwrap();
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

    fn import_expression_artifact(store: &ProjectStore, root: &Path) -> String {
        let source_path = root.join("expression.tsv");
        std::fs::write(&source_path, "gene\tvalue\nTHRSP\t3\n").unwrap();
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

    fn write_auto_run_marker_script(root: &Path) -> PathBuf {
        let script_path = root.join("agent-run-auto-marker.sh");
        std::fs::write(
            &script_path,
            r#"cat "$AGENTFLOW_INPUT_EXPRESSION_TABLE" >/dev/null
printf '# Marker report\nGene: THRSP\nscore: 0.61\n' > "$AGENTFLOW_OUTPUT_MARKER_REPORT"
"#,
        )
        .unwrap();
        script_path
    }

    fn register_exploratory_marker_tool(store: &ProjectStore, script_path: &Path) {
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

    fn write_synthesizer_stub(path: &Path, output: &str) -> String {
        let stub_path = path.join("agent-run-param-synth.sh");
        std::fs::write(
            &stub_path,
            format!(
                "#!/bin/sh\nprintf '%s' '{}'\n",
                output.replace('\'', "'\\''")
            ),
        )
        .unwrap();
        format!("/bin/sh {}", stub_path.display())
    }

    fn write_semantic_synthesizer_stub(path: &Path) -> String {
        let stub_path = path.join("agent-run-semantic-synth.sh");
        std::fs::write(
            &stub_path,
            "#!/bin/sh\ncase \"$*\" in\n  *analysis/latent_assoc*) printf 'yes\\n' ;;\n  *) printf 'no\\n' ;;\nesac\n",
        )
        .unwrap();
        format!("/bin/sh {}", stub_path.display())
    }

    fn write_auto_synthesizer_stub(path: &Path) -> String {
        let stub_path = path.join("agent-run-auto-synth.sh");
        std::fs::write(
            &stub_path,
            r##"#!/bin/sh
cat <<'EOF'
===SCRIPT===
import os

result = "# Auto synth report\nAUTO_SYNTH_OK\n"
output_path = os.environ.get("AGENTFLOW_OUTPUT_RESULT")
if output_path:
    with open(output_path, "w", encoding="utf-8") as handle:
        handle.write(result)
print(result, end="")
===FIXTURE===
marker,value
THRSP,3
===EXPECT===
AUTO_SYNTH_OK
EOF
"##,
        )
        .unwrap();
        format!("/bin/sh {}", stub_path.display())
    }

    fn shell_single_quoted(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    fn option(label: &str) -> HandoffOption {
        HandoffOption {
            label: label.to_string(),
            direction: format!("take {label} path"),
            cost: Cost::Cheap,
            risk: Risk::Low,
            reversible: true,
        }
    }

    fn json_id(output: &str) -> String {
        output
            .split("\"id\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap()
            .to_string()
    }

    #[test]
    fn agent_run_works_with_human_and_json_output() {
        let path = temp_project_path("agent-run-happy");
        let store = init_project(&path);
        let hypothesis_id = record_hypothesis(&store);
        link_weak_evidence(&store, &hypothesis_id);

        let human = run(args(&[
            "agentflow",
            "agent",
            "run",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(human.contains("Agent cycle complete"));
        assert!(human.contains("Outcome: advanced"));
        assert!(human.contains("Provisional verdicts: 1"));
        assert!(human.contains("Branch proposal details:"));
        assert!(human.contains("no tool match"));

        let json = run(args(&[
            "agentflow",
            "agent",
            "run",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(json.contains("\"schema_version\":\"agentflow.agent_cycle.v0\""));
        assert!(json.contains("\"outcome\":\"advanced\""));
        assert!(json.contains("\"matched_tool\":null"));
        assert!(json.contains(&hypothesis_id));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn agent_run_help_mentions_auto_synth() {
        let usage = run(args(&["agentflow", "--help"])).unwrap();
        assert!(usage.contains("[--auto-synth]"));

        let help = run(args(&["agentflow", "agent", "run", "--help"])).unwrap();
        assert!(help.contains("--auto-synth"));
    }

    #[test]
    fn agent_run_auto_synth_registers_runs_and_reports_from_cli() {
        let path = temp_project_path("agent-run-auto-synth");
        let store = init_project(&path);
        let hypothesis_id = record_hypothesis_with_statement(
            &store,
            "Auto synth THRSP pathway validation needs custom validation",
        );
        link_weak_evidence(&store, &hypothesis_id);
        let synthesizer = write_auto_synthesizer_stub(&path);

        let json = run(args(&[
            "agentflow",
            "agent",
            "run",
            "--auto-synth",
            "--synthesizer",
            &synthesizer,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(
            json.contains("\"matched_tool\":\"synth/auto_synth_"),
            "{json}"
        );
        assert!(json.contains("\"matched_fit\":\"synthesized\""));
        assert!(json.contains("auto_synth"));
        assert!(json.contains("stance_assessment"));
        assert_eq!(store.list_observations().unwrap().len(), 1);
        let tools = run(args(&[
            "agentflow",
            "tools",
            "list",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(tools.contains("synth/auto_synth_"));
        assert!(tools.contains("[exploratory]"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn agent_run_semantic_match_promotes_relevant_low_tool_from_stub_backend() {
        let path = temp_project_path("agent-run-semantic-match");
        let store = init_project(&path);
        register_semantic_rerank_tools(&store);
        import_expression_artifact(&store, &path);
        let hypothesis_id =
            record_hypothesis_with_statement(&store, "THRSP survival mechanism needs validation");
        link_weak_evidence(&store, &hypothesis_id);

        let without_semantic = run(args(&[
            "agentflow",
            "agent",
            "run",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(without_semantic.contains("\"matched_tool\":\"analysis/score_low_current\""));
        assert!(without_semantic.contains("\"matched_fit\":\"low\""));
        assert!(!without_semantic.contains("relevance:semantic"));

        let synthesizer = write_semantic_synthesizer_stub(&path);
        let with_semantic = run(args(&[
            "agentflow",
            "agent",
            "run",
            "--semantic-match",
            "--synthesizer",
            &synthesizer,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(with_semantic.contains("\"matched_tool\":\"analysis/latent_assoc\""));
        assert!(with_semantic.contains("\"matched_fit\":\"medium\""));
        assert!(with_semantic.contains("relevance:semantic"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn agent_run_infers_replace_param_from_stub_backend() {
        let path = temp_project_path("agent-run-infer-param");
        let store = init_project(&path);
        register_gene_marker_tool(&store);
        import_expression_artifact(&store, &path);
        let hypothesis_id = record_hypothesis_with_statement(
            &store,
            "Marker THRSP evidence requires deeper pathway validation",
        );
        link_weak_evidence(&store, &hypothesis_id);

        let without_inference = run(args(&[
            "agentflow",
            "agent",
            "run",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(without_inference.contains("\"gene\":\"REPLACE_gene\""));

        let synthesizer = write_synthesizer_stub(&path, "THRSP\n");
        let with_inference = run(args(&[
            "agentflow",
            "agent",
            "run",
            "--infer-params",
            "--synthesizer",
            &synthesizer,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(with_inference.contains("\"gene\":\"THRSP\""));
        assert!(!with_inference.contains("\"gene\":\"REPLACE_gene\""));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn agent_run_auto_forage_links_neutral_evidence_for_provisional_hypothesis() {
        let path = temp_project_path("agent-run-auto-forage-provisional");
        let store = init_project(&path);
        let hypothesis_id = record_hypothesis(&store);
        link_weak_evidence(&store, &hypothesis_id);
        store
            .render_verdict(&hypothesis_id, &RuleBasedEngine, None)
            .unwrap();
        let script = path.join("forage-fixture.sh");
        write_auto_forage_script(&script, None);

        let json = run(args(&[
            "agentflow",
            "agent",
            "run",
            "--auto-forage",
            "--forage-max",
            "2",
            "--forage-script",
            script.to_str().unwrap(),
            "--python",
            "/bin/sh",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(json.contains("\"auto_forage\":{"));
        assert!(json.contains("\"hypotheses_foraged\":1"));
        assert!(json.contains("\"observations_linked\":1"));
        let evidence = store.evidence_for(&hypothesis_id).unwrap();
        let linked = evidence
            .iter()
            .find(|link| link.note == AUTO_FORAGE_NOTE)
            .unwrap();
        assert_eq!(linked.stance, Stance::Neutral);
        assert_eq!(linked.source.as_deref(), Some("PMID:3900002"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn agent_run_auto_forage_skips_strong_verdicts() {
        let path = temp_project_path("agent-run-auto-forage-strong-skip");
        let store = init_project(&path);
        let hypothesis_id = record_hypothesis(&store);
        store
            .link_evidence(EvidenceLinkRequest {
                hypothesis_id: hypothesis_id.clone(),
                observation_id: None,
                source: None,
                grade: EvidenceGrade::Observed,
                stance: Stance::Supports,
                note: "Observed support reaches the decision rule.".to_string(),
            })
            .unwrap();
        store
            .render_verdict(&hypothesis_id, &RuleBasedEngine, Some(valid_gate()))
            .unwrap();
        let script = path.join("forage-fixture.sh");
        write_auto_forage_script(&script, None);

        let output = run(args(&[
            "agentflow",
            "agent",
            "run",
            "--auto-forage",
            "--forage-script",
            script.to_str().unwrap(),
            "--python",
            "/bin/sh",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(output.starts_with("Auto-forage\n"));
        assert!(output.contains("Hypotheses foraged: 0"));
        assert!(output.contains("Observations linked: 0"));
        assert!(!store
            .evidence_for(&hypothesis_id)
            .unwrap()
            .iter()
            .any(|link| link.note == AUTO_FORAGE_NOTE));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn agent_run_auto_forage_skips_single_script_failure_and_continues() {
        let path = temp_project_path("agent-run-auto-forage-skip-failure");
        let store = init_project(&path);
        let failing_statement = "Auto forage fixture should fail";
        let failing_id = record_hypothesis_with_statement(&store, failing_statement);
        let success_id = record_hypothesis_with_statement(&store, "Auto forage fixture succeeds");
        let script = path.join("forage-fixture.sh");
        write_auto_forage_script(&script, Some(failing_statement));

        let json = run(args(&[
            "agentflow",
            "agent",
            "run",
            "--auto-forage",
            "--forage-script",
            script.to_str().unwrap(),
            "--python",
            "/bin/sh",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(json.contains("\"hypotheses_foraged\":1"));
        assert!(json.contains("\"observations_linked\":1"));
        assert!(json.contains(&failing_id));
        assert!(json.contains("status 7"));
        assert!(store
            .evidence_for(&success_id)
            .unwrap()
            .iter()
            .any(|link| link.note == AUTO_FORAGE_NOTE && link.stance == Stance::Neutral));
        assert!(!store
            .evidence_for(&failing_id)
            .unwrap()
            .iter()
            .any(|link| link.note == AUTO_FORAGE_NOTE));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn agent_run_apply_flag_autolands_lifecycle_and_reports_applied_json() {
        let path = temp_project_path("agent-run-apply");
        let store = init_project(&path);
        let hypothesis_id = record_hypothesis(&store);
        link_weak_evidence(&store, &hypothesis_id);

        let json = run(args(&[
            "agentflow",
            "agent",
            "run",
            "--apply",
            "--max-apply",
            "1",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(json.contains("\"applied\":[{"));
        assert!(json.contains("\"type\":\"lifecycle_transition\""));
        assert_eq!(
            store.inspect_hypothesis(&hypothesis_id).unwrap().status,
            HypothesisStatus::UnderTest
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn agent_run_apply_auto_run_raises_stance_assessment_from_cli() {
        let path = temp_project_path("agent-run-auto-run");
        let store = init_project(&path);
        let script = write_auto_run_marker_script(&path);
        register_exploratory_marker_tool(&store, &script);
        let artifact_id = import_expression_artifact(&store, &path);
        approve_auto_run_marker_flow(&store, &artifact_id);
        let statement = "Marker THRSP evidence requires deeper pathway validation";
        let hypothesis_id = record_hypothesis_with_statement(&store, statement);
        link_weak_evidence(&store, &hypothesis_id);

        let json = run(args(&[
            "agentflow",
            "agent",
            "run",
            "--apply",
            "--flow",
            "auto_flow",
            "--auto-run",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(json.contains("\"type\":\"step_run\""));
        assert!(json.contains("\"observation_id\":\"observation_marker_report_"));
        assert!(json.contains("\"outcome\":\"handed_off\""));
        assert!(json.contains("\"kind\":\"stance_assessment\""));
        let observations = store.list_observations().unwrap();
        assert_eq!(observations.len(), 1);
        let observation = &observations[0];
        let pending = store.pending_decision_points().unwrap();
        assert_eq!(pending.len(), 1);
        let point = &pending[0];
        assert_eq!(point.kind, DecisionKind::StanceAssessment);
        assert_eq!(point.recommendation, 2);
        assert!(point.digest.contains(&observation.summary));
        assert!(point.digest.contains(statement));
        assert!(point.digest.contains(&observation.id));
        assert!(point
            .digest
            .contains(&format!("evidence link --hypothesis {hypothesis_id}")));
        assert!(point.digest.contains("--stance supports|contradicts"));
        assert!(point.digest.contains("--grade observed"));
        let evidence = store.evidence_for(&hypothesis_id).unwrap();
        assert!(!evidence.iter().any(|link| link.note == "auto-run"));
        assert!(!evidence
            .iter()
            .any(|link| link.observation_id.as_deref() == Some(observation.id.as_str())));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn branch_candidates_and_select_work_with_json_and_explicit_path() {
        let path = temp_project_path("branch-happy");
        let store = init_project(&path);
        let hypothesis_id = record_hypothesis(&store);

        let candidates = run(args(&[
            "agentflow",
            "branch",
            "candidates",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(candidates.contains("\"schema_version\":\"agentflow.branch_candidates.v0\""));
        assert!(candidates.contains(&hypothesis_id));
        assert!(candidates.contains("\"kind\":\"hold\""));

        let selected = run(args(&[
            "agentflow",
            "branch",
            "select",
            "--explore",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(selected.contains("\"schema_version\":\"agentflow.branch_decisions.v0\""));
        assert!(selected.contains("\"selected_by\":\"explore\""));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn branch_errors_when_project_is_missing() {
        let path = temp_project_path("branch-missing-project");
        let err = run(args(&[
            "agentflow",
            "branch",
            "candidates",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(matches!(err, CliError::Core(_)));
    }

    #[test]
    fn decision_list_show_pending_and_resolve_work_with_json_and_explicit_path() {
        let path = temp_project_path("decision-happy");
        let store = init_project(&path);
        let point = store
            .raise_decision_point(
                DecisionKind::DeepenOrStop,
                "Need user choice before continuing.",
                vec![option("stop"), option("deepen")],
                1,
            )
            .unwrap();

        let list = run(args(&[
            "agentflow",
            "decision",
            "list",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(list.contains("\"schema_version\":\"agentflow.decision_points.v0\""));
        assert!(list.contains(&point.id));

        let pending = run(args(&[
            "agentflow",
            "decision",
            "pending",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(pending.contains("\"status\":\"pending\""));

        let show = run(args(&[
            "agentflow",
            "decision",
            "show",
            &point.id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(show.contains("\"kind\":\"deepen_or_stop\""));

        let resolved = run(args(&[
            "agentflow",
            "decision",
            "resolve",
            &point.id,
            "--choose",
            "1",
            "--note",
            "continue with deeper evidence",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(resolved.contains("\"status\":\"resolved\""));
        assert!(resolved.contains("\"chosen_index\":1"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn decision_resolve_surfaces_core_index_errors() {
        let path = temp_project_path("decision-index-error");
        let store = init_project(&path);
        let point = store
            .raise_decision_point(
                DecisionKind::DeepenOrStop,
                "Need user choice before continuing.",
                vec![option("stop")],
                0,
            )
            .unwrap();

        let err = run(args(&[
            "agentflow",
            "decision",
            "resolve",
            &point.id,
            "--choose",
            "7",
            "--note",
            "bad index",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(matches!(err, CliError::Core(_)));
        assert!(err.message().contains("chosen_index"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn forage_observe_list_show_and_link_work_with_json_and_explicit_path() {
        let path = temp_project_path("forage-happy");
        let store = init_project(&path);
        let hypothesis_id = record_hypothesis(&store);

        let observation = run(args(&[
            "agentflow",
            "forage",
            "observe",
            "--source",
            "pubmed",
            "--external-id",
            "PMID:1",
            "--title",
            "Marker literature",
            "--access",
            "open_access_full_text",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(observation.contains("\"access_status\":\"open_access_full_text\""));
        let observation_id = json_id(&observation);

        let list = run(args(&[
            "agentflow",
            "forage",
            "list",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(list.contains("\"schema_version\":\"agentflow.forage_observations.v0\""));
        assert!(list.contains(&observation_id));

        let show = run(args(&[
            "agentflow",
            "forage",
            "show",
            &observation_id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(show.contains("\"external_id\":\"PMID:1\""));

        let link = run(args(&[
            "agentflow",
            "forage",
            "link",
            "--hypothesis",
            &hypothesis_id,
            "--observation",
            &observation_id,
            "--stance",
            "supports",
            "--note",
            "full text supports the claim",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(link.contains("\"grade\":\"literature_supported\""));
        assert!(link.contains("\"stance\":\"supports\""));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn forage_observe_rejects_invalid_access() {
        let path = temp_project_path("forage-invalid-access");
        let _store = init_project(&path);

        let err = run(args(&[
            "agentflow",
            "forage",
            "observe",
            "--source",
            "pubmed",
            "--external-id",
            "PMID:1",
            "--title",
            "Marker literature",
            "--access",
            "full_text",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(matches!(err, CliError::InvalidArgument(_)));
        assert!(err.message().contains("--access must be"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn forage_ingest_records_fixture_and_skips_blank_lines() {
        let path = temp_project_path("forage-ingest-happy");
        let _store = init_project(&path);
        let hits = path.join("hits.jsonl");
        std::fs::write(
            &hits,
            concat!(
                "{\"external_id\":\"PMID:39000001\",\"title\":\"Marker literature\",\"access_status\":\"abstract_available\"}\n",
                "\n",
                "{\"external_id\":\"PMID:39000002\",\"title\":\"Escaped \\\"title\\\"\",\"access_status\":\"metadata_only\"}\n",
            ),
        )
        .unwrap();

        let output = run(args(&[
            "agentflow",
            "forage",
            "ingest",
            hits.to_str().unwrap(),
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(output.contains("\"schema_version\":\"agentflow.forage_ingest.v0\""));
        assert!(output.contains("\"count\":2"));
        assert!(output.contains("\"observation_ids\":[\"event_"));

        let list = run(args(&[
            "agentflow",
            "forage",
            "list",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(list.contains("\"source_id\":\"pubmed\""));
        assert!(list.contains("\"external_id\":\"PMID:39000001\""));
        assert!(list.contains("\"external_id\":\"PMID:39000002\""));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn forage_ingest_rejects_invalid_access_status() {
        let path = temp_project_path("forage-ingest-invalid-access");
        let _store = init_project(&path);
        let hits = path.join("hits.jsonl");
        std::fs::write(
            &hits,
            "{\"external_id\":\"PMID:1\",\"title\":\"Bad access\",\"access_status\":\"full_text\"}\n",
        )
        .unwrap();

        let err = run(args(&[
            "agentflow",
            "forage",
            "ingest",
            hits.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(matches!(err, CliError::InvalidArgument(_)));
        assert!(err.message().contains("invalid access_status"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn forage_ingest_surfaces_missing_file() {
        let path = temp_project_path("forage-ingest-missing-file");
        let _store = init_project(&path);
        let hits = path.join("missing.jsonl");

        let err = run(args(&[
            "agentflow",
            "forage",
            "ingest",
            hits.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(matches!(err, CliError::Core(_)));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn forage_fetch_validates_required_query_and_max() {
        let path = temp_project_path("forage-fetch-validation");
        let _store = init_project(&path);

        let missing_query = run(args(&[
            "agentflow",
            "forage",
            "fetch",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(matches!(missing_query, CliError::InvalidArgument(_)));
        assert!(missing_query.message().contains("requires --query"));

        let invalid_max = run(args(&[
            "agentflow",
            "forage",
            "fetch",
            "--query",
            "marker",
            "--max",
            "many",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(matches!(invalid_max, CliError::InvalidArgument(_)));
        assert!(invalid_max.message().contains("--max must"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn forage_fetch_surfaces_missing_script_and_nonzero_exit() {
        let path = temp_project_path("forage-fetch-script-errors");
        let _store = init_project(&path);
        let missing_script = path.join("missing.sh");

        let missing_err = run(args(&[
            "agentflow",
            "forage",
            "fetch",
            "--query",
            "marker",
            "--script",
            missing_script.to_str().unwrap(),
            "--python",
            "/bin/sh",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(matches!(missing_err, CliError::InvalidArgument(_)));
        assert!(missing_err.message().contains("script not found"));

        let script = path.join("fail.sh");
        std::fs::write(&script, "echo fixture failed >&2\nexit 9\n").unwrap();
        let nonzero_err = run(args(&[
            "agentflow",
            "forage",
            "fetch",
            "--query",
            "marker",
            "--script",
            script.to_str().unwrap(),
            "--python",
            "/bin/sh",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(matches!(nonzero_err, CliError::Core(_)));
        assert!(nonzero_err.message().contains("status 9"));
        assert!(nonzero_err.message().contains("fixture failed"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn trace_checkpoint_list_drift_and_revert_work_with_explicit_path() {
        let path = temp_project_path("trace-happy");
        let store = init_project(&path);

        let checkpoint = run(args(&[
            "agentflow",
            "trace",
            "checkpoint",
            "--label",
            "baseline",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(checkpoint.contains("\"label\":\"baseline\""));
        let checkpoint_id = json_id(&checkpoint);

        store
            .append_event(EventRecord {
                flow_id: None,
                step_id: None,
                run_id: None,
                event_type: "hypothesis.transitioned".to_string(),
                payload_json: "{}".to_string(),
            })
            .unwrap();

        let list = run(args(&[
            "agentflow",
            "trace",
            "list",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(list.contains("\"schema_version\":\"agentflow.checkpoints.v0\""));
        assert!(list.contains(&checkpoint_id));

        let drift = run(args(&[
            "agentflow",
            "trace",
            "drift",
            &checkpoint_id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(drift.contains("\"autonomous_steps\":1"));

        let revert = run(args(&[
            "agentflow",
            "trace",
            "revert",
            &checkpoint_id,
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(revert.contains("已记录回退"));
        assert!(revert.contains("不物理删除"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn trace_drift_surfaces_missing_checkpoint_error() {
        let path = temp_project_path("trace-missing-checkpoint");
        let _store = init_project(&path);

        let err = run(args(&[
            "agentflow",
            "trace",
            "drift",
            "event_missing",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(matches!(err, CliError::Core(_)));
        assert!(err.message().contains("checkpoint event_missing"));

        let _ = std::fs::remove_dir_all(path);
    }
}
