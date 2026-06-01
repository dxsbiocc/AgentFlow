use std::ffi::OsString;
use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run;
    use agentflow_core::handoff::{Cost, DecisionKind, HandoffOption, Risk};
    use agentflow_core::hypothesis::HypothesisRequest;
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
