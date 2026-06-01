use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use agentflow_core::agent::{AppliedAction, ApplyConfig, CycleReport, EnrichedProposal};
use agentflow_core::argument::{EvidenceLink, Stance};
use agentflow_core::branch::{
    BranchAction, BranchCandidate, BranchDecision, BranchPolicy, CandidateKind, RuleBasedSelector,
    SelectionMode,
};
use agentflow_core::forage::{AccessStatus, ForageObservation};
use agentflow_core::handoff::{DecisionPoint, DecisionStatus};
use agentflow_core::trace_guard::{Checkpoint, DriftReport, RevertRecord};

use crate::{next_arg, require_value, CliError};

#[derive(Debug, Default)]
struct PathJsonOptions {
    path: Option<PathBuf>,
    json: bool,
}

#[derive(Debug)]
struct AgentRunOptions {
    project: PathJsonOptions,
    apply: bool,
    flow: Option<String>,
    max_apply: u32,
}

impl Default for AgentRunOptions {
    fn default() -> Self {
        Self {
            project: PathJsonOptions::default(),
            apply: false,
            flow: None,
            max_apply: 5,
        }
    }
}

#[derive(Debug, Default)]
struct BranchSelectOptions {
    project: PathJsonOptions,
    explore: bool,
}

#[derive(Debug, Default)]
struct SingleIdOptions {
    project: PathJsonOptions,
    id: Option<String>,
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
    max: Option<usize>,
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
const DEFAULT_PUBMED_MAX: usize = 10;
const DEFAULT_PYTHON: &str = "python3";

pub(crate) fn agent_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    match next_arg(&mut args)? {
        Some(command) if command == "run" => agent_run_command(args),
        Some(command) => Err(CliError::InvalidArgument(format!(
            "unknown agent command: {command}"
        ))),
        None => Err(CliError::InvalidArgument(
            "agent requires a command: run".to_string(),
        )),
    }
}

pub(crate) fn branch_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    match next_arg(&mut args)? {
        Some(command) if command == "candidates" => branch_candidates_command(args),
        Some(command) if command == "select" => branch_select_command(args),
        Some(command) => Err(CliError::InvalidArgument(format!(
            "unknown branch command: {command}"
        ))),
        None => Err(CliError::InvalidArgument(
            "branch requires a command: candidates or select".to_string(),
        )),
    }
}

pub(crate) fn decision_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    match next_arg(&mut args)? {
        Some(command) if command == "list" => decision_list_command(args),
        Some(command) if command == "pending" => decision_pending_command(args),
        Some(command) if command == "show" => decision_show_command(args),
        Some(command) if command == "resolve" => decision_resolve_command(args),
        Some(command) => Err(CliError::InvalidArgument(format!(
            "unknown decision command: {command}"
        ))),
        None => Err(CliError::InvalidArgument(
            "decision requires a command: list, pending, show, or resolve".to_string(),
        )),
    }
}

pub(crate) fn forage_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    match next_arg(&mut args)? {
        Some(command) if command == "observe" => forage_observe_command(args),
        Some(command) if command == "ingest" => forage_ingest_command(args),
        Some(command) if command == "fetch" => forage_fetch_command(args),
        Some(command) if command == "list" => forage_list_command(args),
        Some(command) if command == "show" => forage_show_command(args),
        Some(command) if command == "link" => forage_link_command(args),
        Some(command) => Err(CliError::InvalidArgument(format!(
            "unknown forage command: {command}"
        ))),
        None => Err(CliError::InvalidArgument(
            "forage requires a command: observe, list, show, or link".to_string(),
        )),
    }
}

pub(crate) fn trace_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    match next_arg(&mut args)? {
        Some(command) if command == "checkpoint" => trace_checkpoint_command(args),
        Some(command) if command == "list" => trace_list_command(args),
        Some(command) if command == "drift" => trace_drift_command(args),
        Some(command) if command == "revert" => trace_revert_command(args),
        Some(command) => Err(CliError::InvalidArgument(format!(
            "unknown trace command: {command}"
        ))),
        None => Err(CliError::InvalidArgument(
            "trace requires a command: checkpoint, list, drift, or revert".to_string(),
        )),
    }
}

fn agent_run_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_agent_run_options(args)?;
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let report = store.run_cycle_with_apply_config(ApplyConfig {
        apply: options.apply,
        flow: options.flow,
        max_apply: options.max_apply,
    })?;

    if options.project.json {
        Ok(report.to_json())
    } else {
        Ok(format_cycle_report(&report))
    }
}

fn branch_candidates_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_path_json_options(args, "branch candidates")?;
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

fn branch_select_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_branch_select_options(args)?;
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

fn decision_list_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_path_json_options(args, "decision list")?;
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

fn decision_pending_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_path_json_options(args, "decision pending")?;
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

fn decision_show_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_id_options(args, "decision id")?;
    let decision_id = options.id.ok_or_else(|| {
        CliError::InvalidArgument("decision show requires <decision-id>".to_string())
    })?;
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let point = store.inspect_decision_point(&decision_id)?;

    if options.project.json {
        Ok(point.to_json())
    } else {
        Ok(format_decision_point("Decision point", &point))
    }
}

fn decision_resolve_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_decision_resolve_options(args)?;
    let decision_id = options.decision_id.ok_or_else(|| {
        CliError::InvalidArgument("decision resolve requires <decision-id>".to_string())
    })?;
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

fn forage_observe_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_forage_observe_options(args)?;
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

fn forage_ingest_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_forage_ingest_options(args)?;
    let hits_file = options.hits_file.ok_or_else(|| {
        CliError::InvalidArgument("forage ingest requires <hits-file>".to_string())
    })?;
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

fn forage_fetch_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_forage_fetch_options(args)?;
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
    if script.as_os_str().is_empty() {
        return Err(CliError::InvalidArgument(
            "forage fetch requires --script".to_string(),
        ));
    }
    if !script.exists() {
        return Err(CliError::InvalidArgument(format!(
            "forage fetch script not found: {}",
            script.display()
        )));
    }

    let python = options.python.unwrap_or_else(|| DEFAULT_PYTHON.to_string());
    if python.trim().is_empty() {
        return Err(CliError::InvalidArgument(
            "forage fetch requires --python".to_string(),
        ));
    }

    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let out_file = forage_fetch_tmp_path();
    let max = options.max.unwrap_or(DEFAULT_PUBMED_MAX);
    let output = Command::new(&python)
        .arg(&script)
        .arg("--query")
        .arg(&query)
        .arg("--max")
        .arg(max.to_string())
        .arg("--out")
        .arg(&out_file)
        .output()
        .map_err(|error| {
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

    let observations = ingest_forage_hits(&store, &out_file, &source);
    let _ = std::fs::remove_file(&out_file);
    let observations = observations?;

    if options.project.json {
        Ok(forage_ingest_summary_json(&observations))
    } else {
        Ok(format_forage_ingest_summary(&observations))
    }
}

fn forage_list_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_path_json_options(args, "forage list")?;
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

fn forage_show_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_id_options(args, "forage observation id")?;
    let observation_id = options.id.ok_or_else(|| {
        CliError::InvalidArgument("forage show requires <forage-obs-id>".to_string())
    })?;
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let observation = store.inspect_forage_observation(&observation_id)?;

    if options.project.json {
        Ok(observation.to_json())
    } else {
        Ok(format_forage_observation(
            "Forage observation",
            &observation,
        ))
    }
}

fn forage_link_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_forage_link_options(args)?;
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

fn trace_checkpoint_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_trace_checkpoint_options(args)?;
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

fn trace_list_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_path_json_options(args, "trace list")?;
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

fn trace_drift_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_id_options(args, "checkpoint id")?;
    let checkpoint_id = options.id.ok_or_else(|| {
        CliError::InvalidArgument("trace drift requires <checkpoint-id>".to_string())
    })?;
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let report = store.detect_drift(&checkpoint_id)?;

    if options.project.json {
        Ok(report.to_json())
    } else {
        Ok(format_drift_report(&report))
    }
}

fn trace_revert_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_id_options(args, "checkpoint id")?;
    let checkpoint_id = options.id.ok_or_else(|| {
        CliError::InvalidArgument("trace revert requires <checkpoint-id>".to_string())
    })?;
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let record = store.revert_to(&checkpoint_id)?;

    if options.project.json {
        Ok(record.to_json())
    } else {
        Ok(format_revert_record(&record))
    }
}

fn parse_path_json_options<I>(args: I, command: &str) -> Result<PathJsonOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = PathJsonOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--json" => {
                options.json = true;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                return Err(CliError::InvalidArgument(format!(
                    "{command} does not accept positional argument: {arg}"
                )));
            }
        }
    }

    Ok(options)
}

fn parse_agent_run_options<I>(args: I) -> Result<AgentRunOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = AgentRunOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project.path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--json" => {
                options.project.json = true;
            }
            "--apply" => {
                options.apply = true;
            }
            "--flow" => {
                options.flow = Some(require_value("--flow", &mut args)?);
            }
            "--max-apply" => {
                let max_apply = parse_usize_flag("--max-apply", &mut args)?;
                options.max_apply = u32::try_from(max_apply).map_err(|_| {
                    CliError::InvalidArgument(
                        "--max-apply must fit in an unsigned 32-bit integer".to_string(),
                    )
                })?;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                return Err(CliError::InvalidArgument(format!(
                    "agent run does not accept positional argument: {arg}"
                )));
            }
        }
    }

    Ok(options)
}

fn parse_branch_select_options<I>(args: I) -> Result<BranchSelectOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = BranchSelectOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project.path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--json" => {
                options.project.json = true;
            }
            "--explore" => {
                options.explore = true;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                return Err(CliError::InvalidArgument(format!(
                    "branch select does not accept positional argument: {arg}"
                )));
            }
        }
    }

    Ok(options)
}

fn parse_single_id_options<I>(args: I, label: &str) -> Result<SingleIdOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = SingleIdOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project.path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--json" => {
                options.project.json = true;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                if options.id.is_some() {
                    return Err(CliError::InvalidArgument(format!(
                        "expected one {label}, got extra argument: {arg}"
                    )));
                }
                options.id = Some(arg);
            }
        }
    }

    Ok(options)
}

fn parse_decision_resolve_options<I>(args: I) -> Result<DecisionResolveOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = DecisionResolveOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project.path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--json" => {
                options.project.json = true;
            }
            "--choose" => {
                options.choose = Some(parse_usize_flag("--choose", &mut args)?);
            }
            "--note" => {
                options.note = Some(require_value("--note", &mut args)?);
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                if options.decision_id.is_some() {
                    return Err(CliError::InvalidArgument(format!(
                        "expected one decision id, got extra argument: {arg}"
                    )));
                }
                options.decision_id = Some(arg);
            }
        }
    }

    Ok(options)
}

fn parse_forage_observe_options<I>(args: I) -> Result<ForageObserveOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = ForageObserveOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project.path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--json" => {
                options.project.json = true;
            }
            "--source" => {
                options.source = Some(require_value("--source", &mut args)?);
            }
            "--external-id" => {
                options.external_id = Some(require_value("--external-id", &mut args)?);
            }
            "--title" => {
                options.title = Some(require_value("--title", &mut args)?);
            }
            "--access" => {
                let access = require_value("--access", &mut args)?;
                options.access = Some(parse_access_status(&access)?);
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                return Err(CliError::InvalidArgument(format!(
                    "forage observe does not accept positional argument: {arg}"
                )));
            }
        }
    }

    Ok(options)
}

fn parse_forage_ingest_options<I>(args: I) -> Result<ForageIngestOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = ForageIngestOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project.path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--json" => {
                options.project.json = true;
            }
            "--source" => {
                options.source = Some(require_value("--source", &mut args)?);
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                if options.hits_file.is_some() {
                    return Err(CliError::InvalidArgument(format!(
                        "expected one hits file, got extra argument: {arg}"
                    )));
                }
                options.hits_file = Some(PathBuf::from(arg));
            }
        }
    }

    Ok(options)
}

fn parse_forage_fetch_options<I>(args: I) -> Result<ForageFetchOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = ForageFetchOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project.path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--json" => {
                options.project.json = true;
            }
            "--query" => {
                options.query = Some(require_value("--query", &mut args)?);
            }
            "--source" => {
                options.source = Some(require_value("--source", &mut args)?);
            }
            "--script" => {
                options.script = Some(PathBuf::from(require_value("--script", &mut args)?));
            }
            "--max" => {
                options.max = Some(parse_usize_flag("--max", &mut args)?);
            }
            "--python" => {
                options.python = Some(require_value("--python", &mut args)?);
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                return Err(CliError::InvalidArgument(format!(
                    "forage fetch does not accept positional argument: {arg}"
                )));
            }
        }
    }

    Ok(options)
}

fn parse_forage_link_options<I>(args: I) -> Result<ForageLinkOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = ForageLinkOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project.path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--json" => {
                options.project.json = true;
            }
            "--hypothesis" => {
                options.hypothesis_id = Some(require_value("--hypothesis", &mut args)?);
            }
            "--observation" => {
                options.observation_id = Some(require_value("--observation", &mut args)?);
            }
            "--stance" => {
                let stance = require_value("--stance", &mut args)?;
                options.stance = Some(parse_stance(&stance)?);
            }
            "--note" => {
                options.note = Some(require_value("--note", &mut args)?);
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                return Err(CliError::InvalidArgument(format!(
                    "forage link does not accept positional argument: {arg}"
                )));
            }
        }
    }

    Ok(options)
}

fn parse_trace_checkpoint_options<I>(args: I) -> Result<TraceCheckpointOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = TraceCheckpointOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project.path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--json" => {
                options.project.json = true;
            }
            "--label" => {
                options.label = Some(require_value("--label", &mut args)?);
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                return Err(CliError::InvalidArgument(format!(
                    "trace checkpoint does not accept positional argument: {arg}"
                )));
            }
        }
    }

    Ok(options)
}

fn parse_usize_flag<I>(flag: &str, args: &mut I) -> Result<usize, CliError>
where
    I: Iterator<Item = OsString>,
{
    let value = require_value(flag, args)?;
    value
        .parse::<usize>()
        .map_err(|_| CliError::InvalidArgument(format!("{flag} must be a non-negative integer")))
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
    use super::*;
    use crate::run;
    use agentflow_core::argument::{EvidenceGrade, EvidenceLinkRequest};
    use agentflow_core::handoff::{Cost, DecisionKind, HandoffOption, Risk};
    use agentflow_core::hypothesis::{HypothesisRequest, HypothesisStatus};
    use agentflow_core::storage::{EventRecord, ProjectStore};

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
