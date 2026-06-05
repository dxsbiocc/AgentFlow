mod agent_commands;
mod agent_ops_commands;
mod cli_args;
mod llm_commands;
mod synth_commands;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli_args::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    Core(String),
    InvalidArgument(String),
    UnknownCommand(String),
}

impl CliError {
    pub fn message(&self) -> String {
        match self {
            Self::Core(message) | Self::InvalidArgument(message) => message.clone(),
            Self::UnknownCommand(command) => {
                format!("unknown command: {command}\n\n{}", usage())
            }
        }
    }
}

impl From<agentflow_core::storage::StorageError> for CliError {
    fn from(error: agentflow_core::storage::StorageError) -> Self {
        Self::Core(error.to_string())
    }
}

impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        Self::Core(error.to_string())
    }
}

pub fn run<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    cli_args::run(args)
}

pub fn usage() -> String {
    [
        "agentflow - CLI-first local runtime for AgentFlow",
        "",
        "Usage:",
        "  agentflow --version",
        "  agentflow help",
        "  agentflow init [--name <name>] [--path <path>]",
        "  agentflow status [--json] [--path <path>]",
        "  agentflow doctor [--path <path>]",
        "  agentflow tools register <tool.yaml> [--path <path>]",
        "  agentflow tools list [--json] [--path <path>]",
        "  agentflow tools inspect <tool-ref> [--json] [--path <path>]",
        "  agentflow tools match [--output <type>] [--input <type>]... [--keyword <kw>]... [--json] [--path <path>]",
        "  agentflow tools draft-step <tool-ref> [--input <type>:<artifact-id>]... [--hypothesis <id>] [--infer-params] [--synthesizer <cmd>] [--json] [--path <path>]",
        "  agentflow synth --name <n> --description <text> --fixture <input-file> --expect <substring> [--synthesizer <cmd>] [--path <path>]",
        "  agentflow llm config --provider anthropic|openai|gemini|deepseek (--api-key <key>|--api-key-env <env-var>) [--model <model>] [--base-url <url>] [--synthesizer <cmd>] [--json] [--path <path>]",
        "  agentflow env check <tool-ref> [--json] [--path <path>]",
        "  agentflow env prepare <tool-ref> [--json] [--path <path>]",
        "  agentflow env export <tool-ref> [--json] [--path <path>]",
        "  agentflow import <file> --type <artifact-type> [--mode reference|copy] [--path <path>]",
        "  agentflow artifacts list [--json] [--path <path>]",
        "  agentflow artifacts inspect <artifact-id> [--json] [--path <path>]",
        "  agentflow flow validate <flow.yaml> [--json] [--path <path>]",
        "  agentflow flow approve <flow.yaml> [--path <path>]",
        "  agentflow flow inspect <flow-id> [--json] [--path <path>]",
        "  agentflow run <flow-id> [--path <path>]",
        "  agentflow run-step <step-id|flow.step|step:flow/step> [--path <path>]",
        "  agentflow report <flow-id> [--path <path>]",
        "  agentflow report research [--path <path>]",
        "  agentflow cache explain <flow-id|step-id> [--path <path>]",
        "  agentflow cache list [--json] [--path <path>]",
        "  agentflow cache prune (--all|--older-than-seconds <seconds>) [--json] [--path <path>]",
        "  agentflow retry <step-id|flow.step|step:flow/step> [--path <path>]",
        "  agentflow observe <artifact-id> [--adapter artifact_summary|marker_report] [--json] [--path <path>]",
        "  agentflow observations list [--json] [--path <path>]",
        "  agentflow observations inspect <observation-id> [--json] [--path <path>]",
        "  agentflow research note --problem <text> --question <text> --finding <text> [--confidence low|medium|high] [--source <text>] [--path <path>]",
        "  agentflow research list [--json] [--path <path>]",
        "  agentflow research inspect <note-id> [--json] [--path <path>]",
        "  agentflow hypothesis create --statement <text> --origin <text> --goal <goal-id> [--json] [--path <path>]",
        "  agentflow hypothesis list [--json] [--path <path>]",
        "  agentflow hypothesis show <hypothesis-id> [--json] [--path <path>]",
        "  agentflow hypothesis transition <hypothesis-id> --to <status> [--confidence low|medium|high] [--json] [--path <path>]",
        "  agentflow evidence link --hypothesis <id> --grade observed|inferred|literature_supported|hypothesis|unsupported --stance supports|contradicts|neutral --note <text> [--observation <obs-id>] [--source <text>] [--json] [--path <path>]",
        "  agentflow evidence list --hypothesis <id> [--json] [--path <path>]",
        "  agentflow verdict render --hypothesis <id> [--json] [--path <path>] [--gate-supports <text> --gate-against <text> --gate-alternatives <text> --gate-data-risks <text> --gate-assumptions <text> --gate-falsifier <text> --gate-claim-basis observed|inferred|speculative --gate-not-yet <text>]",
        "  agentflow verdict show --hypothesis <id> [--json] [--path <path>]",
        "  agentflow agent run [--apply] [--no-apply] [--auto-run] [--no-auto-run] [--dry-run] [--flow <flow-id>] [--max-apply <n>] [--propose-synth] [--auto-synth] [--no-auto-synth] [--infer-params] [--no-infer-params] [--semantic-match] [--no-semantic-match] [--synthesizer <cmd>] [--auto-forage] [--no-auto-forage] [--forage-max <n>] [--forage-script <path>] [--python <bin>] [--json] [--path <path>]",
        "  agentflow branch candidates [--json] [--path <path>]",
        "  agentflow branch select [--explore] [--json] [--path <path>]",
        "  agentflow decision list [--json] [--path <path>]",
        "  agentflow decision pending [--json] [--path <path>]",
        "  agentflow decision show <decision-id> [--json] [--path <path>]",
        "  agentflow decision resolve <decision-id> --choose <index> --note <text> [--json] [--path <path>]",
        "  agentflow forage observe --source <source> --external-id <external-id> --title <title> --access metadata_only|abstract_available|open_access_full_text|user_provided_full_text|subscription_connector_full_text|full_text_unavailable|retrieval_failed [--json] [--path <path>]",
        "  agentflow forage list [--json] [--path <path>]",
        "  agentflow forage show <forage-obs-id> [--json] [--path <path>]",
        "  agentflow forage link --hypothesis <id> --observation <forage-obs-id> --stance supports|contradicts|neutral --note <text> [--json] [--path <path>]",
        "  agentflow trace checkpoint --label <text> [--json] [--path <path>]",
        "  agentflow trace list [--json] [--path <path>]",
        "  agentflow trace drift <checkpoint-id> [--json] [--path <path>]",
        "  agentflow trace revert <checkpoint-id> [--json] [--path <path>]",
        "  agentflow patch propose <flow-id> --title <text> --reason <text> (--patch-json <json>|--patch-file <file>) [--json] [--path <path>]",
        "  agentflow patch list <flow-id> [--json] [--path <path>]",
        "  agentflow patch approve <patch-id> [--json] [--path <path>]",
        "  agentflow patch reject <patch-id> --reason <text> [--json] [--path <path>]",
        "  agentflow patch apply <patch-id> [--json] [--path <path>]",
        "  agentflow compare steps <flow-id> --baseline <step-id> --candidate <step-id> --summary <text> [--winner baseline|candidate|tie|inconclusive] [--reason <text>] [--json] [--path <path>]",
        "  agentflow compare metrics <flow-id> --baseline <step-id> --candidate <step-id> --metric <name> [--direction higher|lower] [--json] [--path <path>]",
        "  agentflow compare list <flow-id> [--json] [--path <path>]",
        "  agentflow compare inspect <comparison-id> [--json] [--path <path>]",
        "  agentflow runs list [--flow <flow-id>] [--json] [--path <path>]",
        "  agentflow runs inspect <run-or-attempt-id> [--json] [--path <path>]",
        "  agentflow logs <run-or-attempt-id> [--path <path>]",
        "",
        "Implementation status:",
        "  V1 usable CLI runtime slice is available for approved executable flows.",
    ]
    .join("\n")
}

fn init_command(args: InitArgs) -> Result<String, CliError> {
    let options = ProjectOptions::from(args);
    let path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::init(&path, options.name.as_deref())?;
    let summary = store.summary()?;
    Ok(format!(
        "Initialized AgentFlow project\nName: {}\nPath: {}\nDatabase: {}",
        summary.name,
        summary.root_path.display(),
        agentflow_core::storage::project_db_path(&summary.root_path).display()
    ))
}

fn status_command(args: PathJsonArgs) -> Result<String, CliError> {
    let options = ProjectOptions::from(args);
    let path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let summary = store.summary()?;
    if options.json {
        Ok(store.status_json()?)
    } else {
        Ok(format!(
            "AgentFlow project\nName: {}\nPath: {}\nEngine: {}\nCreated: {}\nUpdated: {}",
            summary.name,
            summary.root_path.display(),
            summary.engine_version,
            summary.created_at,
            summary.updated_at
        ))
    }
}

fn run_command(args: RunArgs) -> Result<String, CliError> {
    let flow_id = args.flow_id;
    let project_path = project_path_from_only(args.project)?;
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let summary = store.run_flow(&flow_id)?;
    Ok(format!(
        "Run complete\nFlow: {}\nCompleted steps: {}\nFailed steps: {}\nAttempts:\n{}",
        summary.flow_id,
        summary.completed_steps,
        summary.failed_steps,
        format_attempts(&summary.attempts)
    ))
}

fn run_step_command(args: StepRefArgs) -> Result<String, CliError> {
    let step_id = args.step_id;
    let project_path = project_path_from_only(args.project)?;
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let summary = store.run_step_ref(&step_id)?;
    Ok(format!(
        "Run step complete\nFlow: {}\nCompleted steps: {}\nFailed steps: {}\nAttempts:\n{}",
        summary.flow_id,
        summary.completed_steps,
        summary.failed_steps,
        format_attempts(&summary.attempts)
    ))
}

fn runs_command(args: RunsArgs) -> Result<String, CliError> {
    match args.command {
        RunsCommand::List(args) => runs_list_command(args),
        RunsCommand::Inspect(args) => runs_inspect_command(args),
    }
}

fn runs_list_command(args: RunsListArgs) -> Result<String, CliError> {
    let options = RunsListOptions {
        project: ProjectOptions::from(args.project),
        flow_id: last_value(args.flow),
    };
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let runs = store.list_runs(options.flow_id.as_deref())?;
    if options.project.json {
        Ok(runs_json(&runs))
    } else if runs.is_empty() {
        Ok("Runs\n_none_".to_string())
    } else {
        Ok(format!("Runs\n{}", format_runs(&runs)))
    }
}

fn runs_inspect_command(args: RunsInspectArgs) -> Result<String, CliError> {
    let id = args.id;
    let options = ProjectOptions::from(args.project);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let inspection = store.inspect_run_or_attempt(&id)?;
    if options.json {
        Ok(run_inspection_json(&inspection))
    } else {
        Ok(format!(
            "Run: {}\nFlow: {}\nStep: {}\nStatus: {}\nAttempts:\n{}",
            inspection.run.run_id,
            inspection.run.flow_id,
            inspection.run.step_id,
            inspection.run.status,
            format_run_attempt_records(&inspection.attempts)
        ))
    }
}

fn logs_command(args: LogsArgs) -> Result<String, CliError> {
    let id = args.id;
    let project_path = project_path_from_only(args.project)?;
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let logs = store.read_logs(&id)?;
    Ok(format!(
        "Attempt: {}\nStdout: {}\n{}\nStderr: {}\n{}",
        logs.attempt_id,
        logs.stdout_path.display(),
        logs.stdout,
        logs.stderr_path.display(),
        logs.stderr
    ))
}

fn report_command(args: ReportArgs) -> Result<String, CliError> {
    let flow_id = args.flow_id;
    let project_path = project_path_from_only(args.project)?;
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    if flow_id == "research" {
        store
            .generate_research_report_markdown()
            .map_err(Into::into)
    } else {
        store.generate_report_markdown(&flow_id).map_err(Into::into)
    }
}

fn cache_command(args: CacheArgs) -> Result<String, CliError> {
    match args.command {
        CacheCommand::Explain(args) => cache_explain_command(args),
        CacheCommand::List(args) => cache_list_command(args),
        CacheCommand::Prune(args) => cache_prune_command(args),
    }
}

fn cache_explain_command(args: CacheExplainArgs) -> Result<String, CliError> {
    let target = args.target;
    let project_path = project_path_from_only(args.project)?;
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;

    let explanations = store.cache_explain_target(&target)?;
    Ok(format_cache_explanations(&target, &explanations))
}

fn cache_list_command(args: PathJsonArgs) -> Result<String, CliError> {
    let options = ProjectOptions::from(args);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let entries = store.list_cache_entries()?;
    if options.json {
        Ok(cache_entries_json(&entries))
    } else if entries.is_empty() {
        Ok("Cache entries\n_none_".to_string())
    } else {
        Ok(format!(
            "Cache entries\n{}",
            entries
                .iter()
                .map(|entry| {
                    format!(
                        "{} {}\n  outputs: {}\n  created_at: {}\n  last_used_at: {}",
                        entry.cache_key,
                        entry.tool_ref,
                        entry.output_count,
                        entry.created_at,
                        entry.last_used_at
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        ))
    }
}

fn cache_prune_command(args: CachePruneArgs) -> Result<String, CliError> {
    let options = CachePruneOptions::try_from(args)?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let summary = store.prune_cache_entries(options.older_than_seconds)?;
    if options.project.json {
        Ok(format!(
            "{{\"schema_version\":\"agentflow.cache_prune.v0\",\"removed_entries\":{}}}",
            summary.removed_entries
        ))
    } else {
        Ok(format!(
            "Cache prune complete\nRemoved entries: {}",
            summary.removed_entries
        ))
    }
}

fn retry_command(args: StepRefArgs) -> Result<String, CliError> {
    let step_id = args.step_id;
    let project_path = project_path_from_only(args.project)?;
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let summary = store.retry_step_ref(&step_id)?;
    Ok(format!(
        "Retry complete\nFlow: {}\nCompleted steps: {}\nFailed steps: {}\nAttempts:\n{}",
        summary.flow_id,
        summary.completed_steps,
        summary.failed_steps,
        format_attempts(&summary.attempts)
    ))
}

fn observe_command(args: ObserveArgs) -> Result<String, CliError> {
    let options = ObserveOptions {
        project_path: last_value(args.project.path),
        artifact_id: Some(args.artifact_id),
        adapter: last_value(args.adapter),
        json: args.project.json,
    };
    let artifact_id = options.artifact_id.expect("clap requires artifact id");
    let project_path = options.project_path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let observation = match options.adapter.as_deref() {
        Some(adapter) => store.observe_artifact_with_adapter(&artifact_id, adapter)?,
        None => store.observe_artifact(&artifact_id)?,
    };

    if options.json {
        Ok(observation_json(&observation))
    } else {
        Ok(format_observation(&observation))
    }
}

fn observations_command(args: ObservationsArgs) -> Result<String, CliError> {
    match args.command {
        ObservationsCommand::List(args) => observations_list_command(args),
        ObservationsCommand::Inspect(args) => observations_inspect_command(args),
    }
}

fn observations_list_command(args: PathJsonArgs) -> Result<String, CliError> {
    let options = ProjectOptions::from(args);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let observations = store.list_observations()?;

    if options.json {
        Ok(observations_json(&observations))
    } else if observations.is_empty() {
        Ok("No observations recorded".to_string())
    } else {
        Ok(observations
            .iter()
            .map(|observation| {
                format!(
                    "{} [{}:{}] {}",
                    observation.id, observation.kind, observation.severity, observation.summary
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn observations_inspect_command(args: ObservationInspectArgs) -> Result<String, CliError> {
    let observation_id = args.observation_id;
    let options = ProjectOptions::from(args.project);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let observation = store.inspect_observation(&observation_id)?;

    if options.json {
        Ok(observation_json(&observation))
    } else {
        Ok(format_observation(&observation))
    }
}

fn research_command(args: ResearchArgs) -> Result<String, CliError> {
    match args.command {
        ResearchCommand::Note(args) => research_note_command(args),
        ResearchCommand::List(args) => research_list_command(args),
        ResearchCommand::Inspect(args) => research_inspect_command(args),
    }
}

fn research_note_command(args: ResearchNoteArgs) -> Result<String, CliError> {
    let options = ResearchNoteOptions {
        project_path: last_value(args.project.path),
        problem: last_value(args.problem),
        question: last_value(args.question),
        finding: last_value(args.finding),
        confidence: last_value(args.confidence),
        source: last_value(args.source),
    };
    let problem = options
        .problem
        .ok_or_else(|| CliError::InvalidArgument("research note requires --problem".to_string()))?;
    let question = options.question.ok_or_else(|| {
        CliError::InvalidArgument("research note requires --question".to_string())
    })?;
    let finding = options
        .finding
        .ok_or_else(|| CliError::InvalidArgument("research note requires --finding".to_string()))?;
    let project_path = options.project_path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let note = store.record_research_note(agentflow_core::research::ResearchNoteRequest {
        problem,
        question,
        finding,
        confidence: options.confidence.unwrap_or_else(|| "medium".to_string()),
        source: options.source,
    })?;
    Ok(format!(
        "Recorded research note\nId: {}\nConfidence: {}\nQuestion: {}\nFinding: {}",
        note.id, note.confidence, note.question, note.finding
    ))
}

fn research_list_command(args: PathJsonArgs) -> Result<String, CliError> {
    let options = ProjectOptions::from(args);
    let path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let notes = store.list_research_notes()?;
    if options.json {
        Ok(research_notes_json(&notes))
    } else if notes.is_empty() {
        Ok("No research notes recorded".to_string())
    } else {
        Ok(notes
            .iter()
            .map(|note| {
                format!(
                    "{} [{}] {}\n  finding: {}",
                    note.id, note.confidence, note.question, note.finding
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn research_inspect_command(args: ResearchInspectArgs) -> Result<String, CliError> {
    let note_id = args.note_id;
    let options = ProjectOptions::from(args.project);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let note = store.inspect_research_note(&note_id)?;
    if options.json {
        Ok(note.to_json())
    } else {
        Ok(format!(
            "Research note: {}\nProblem: {}\nQuestion: {}\nFinding: {}\nConfidence: {}\nSource: {}\nCreated: {}",
            note.id,
            note.problem,
            note.question,
            note.finding,
            note.confidence,
            note.source.unwrap_or_else(|| "none".to_string()),
            note.created_at
        ))
    }
}

fn patch_command(args: PatchArgs) -> Result<String, CliError> {
    match args.command {
        PatchCommand::Propose(args) => patch_propose_command(args),
        PatchCommand::List(args) => patch_list_command(args),
        PatchCommand::Approve(args) => patch_approve_command(args),
        PatchCommand::Reject(args) => patch_reject_command(args),
        PatchCommand::Apply(args) => patch_apply_command(args),
    }
}

fn patch_propose_command(args: PatchProposeArgs) -> Result<String, CliError> {
    let options = GraphPatchProposeOptions {
        project_path: last_value(args.project.path),
        flow_id: Some(args.flow_id),
        title: last_value(args.title),
        reason: last_value(args.reason),
        patch_json: last_value(args.patch_json),
        patch_file: last_value(args.patch_file),
        json: args.project.json,
    };
    let flow_id = options.flow_id.expect("clap requires flow id");
    let title = options
        .title
        .ok_or_else(|| CliError::InvalidArgument("patch propose requires --title".to_string()))?;
    let reason = options
        .reason
        .ok_or_else(|| CliError::InvalidArgument("patch propose requires --reason".to_string()))?;
    let patch_json = match (options.patch_json, options.patch_file) {
        (Some(value), None) => value,
        (None, Some(path)) => fs::read_to_string(path)?,
        (None, None) => {
            return Err(CliError::InvalidArgument(
                "patch propose requires --patch-json or --patch-file".to_string(),
            ));
        }
        (Some(_), Some(_)) => {
            return Err(CliError::InvalidArgument(
                "use either --patch-json or --patch-file, not both".to_string(),
            ));
        }
    };
    let project_path = options.project_path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let patch = store.propose_graph_patch(&flow_id, &title, &reason, &patch_json)?;

    if options.json {
        Ok(graph_patch_json(&patch))
    } else {
        Ok(format_graph_patch("Proposed graph patch", &patch))
    }
}

fn patch_list_command(args: PatchListArgs) -> Result<String, CliError> {
    let flow_id = args.flow_id;
    let options = ProjectOptions::from(args.project);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let patches = store.list_graph_patches(&flow_id)?;

    if options.json {
        Ok(graph_patches_json(&flow_id, &patches))
    } else if patches.is_empty() {
        Ok(format!("No graph patches recorded for flow {flow_id}"))
    } else {
        Ok(patches
            .iter()
            .map(|patch| {
                format!(
                    "{} [{}] {}\n  reason: {}",
                    patch.id, patch.status, patch.title, patch.reason
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn patch_approve_command(args: PatchIdArgs) -> Result<String, CliError> {
    let patch_id = args.patch_id;
    let options = ProjectOptions::from(args.project);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let patch = store.approve_graph_patch(&patch_id)?;

    if options.json {
        Ok(graph_patch_json(&patch))
    } else {
        Ok(format_graph_patch("Approved graph patch", &patch))
    }
}

fn patch_reject_command(args: PatchRejectArgs) -> Result<String, CliError> {
    let options = GraphPatchRejectOptions {
        project_path: last_value(args.project.path),
        patch_id: Some(args.patch_id),
        reason: last_value(args.reason),
        json: args.project.json,
    };
    let patch_id = options.patch_id.expect("clap requires patch id");
    let reason = options
        .reason
        .ok_or_else(|| CliError::InvalidArgument("patch reject requires --reason".to_string()))?;
    let project_path = options.project_path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let patch = store.reject_graph_patch(&patch_id, &reason)?;

    if options.json {
        Ok(graph_patch_json(&patch))
    } else {
        Ok(format_graph_patch("Rejected graph patch", &patch))
    }
}

fn patch_apply_command(args: PatchIdArgs) -> Result<String, CliError> {
    let patch_id = args.patch_id;
    let options = ProjectOptions::from(args.project);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let application = store.apply_graph_patch(&patch_id)?;

    if options.json {
        Ok(graph_patch_application_json(&application))
    } else {
        Ok(format!(
            "Applied graph patch\nId: {}\nFlow: {}\nApplied steps: {}\nApplied edges: {}\nUpdated steps: {}\nInvalidated steps: {}",
            application.patch_id,
            application.flow_id,
            application.applied_steps.join(", "),
            application
                .applied_edges
                .iter()
                .map(|(from, to)| format!("{from}->{to}"))
                .collect::<Vec<_>>()
                .join(", "),
            application.updated_steps.join(", "),
            application.invalidated_steps.join(", ")
        ))
    }
}

fn compare_command(args: CompareArgs) -> Result<String, CliError> {
    match args.command {
        CompareCommand::Steps(args) => compare_steps_command(args),
        CompareCommand::Metrics(args) => compare_metrics_command(args),
        CompareCommand::List(args) => compare_list_command(args),
        CompareCommand::Inspect(args) => compare_inspect_command(args),
    }
}

fn compare_steps_command(args: CompareStepsArgs) -> Result<String, CliError> {
    let options = CompareStepsOptions {
        project_path: last_value(args.project.path),
        flow_id: Some(args.flow_id),
        baseline_step: last_value(args.baseline),
        candidate_step: last_value(args.candidate),
        summary: last_value(args.summary),
        winner: last_value(args.winner),
        reason: last_value(args.reason),
        json: args.project.json,
    };
    let flow_id = options.flow_id.expect("clap requires flow id");
    let baseline_step = options.baseline_step.ok_or_else(|| {
        CliError::InvalidArgument("compare steps requires --baseline".to_string())
    })?;
    let candidate_step = options.candidate_step.ok_or_else(|| {
        CliError::InvalidArgument("compare steps requires --candidate".to_string())
    })?;
    let summary = options
        .summary
        .ok_or_else(|| CliError::InvalidArgument("compare steps requires --summary".to_string()))?;
    let project_path = options.project_path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let comparison =
        store.record_branch_comparison(agentflow_core::comparison::BranchComparisonRequest {
            flow_id,
            baseline_step,
            candidate_step,
            summary,
            winner: options.winner,
            reason: options.reason,
        })?;

    if options.json {
        Ok(comparison.to_json())
    } else {
        Ok(format_branch_comparison(
            "Recorded branch comparison",
            &comparison,
        ))
    }
}

fn compare_metrics_command(args: CompareMetricsArgs) -> Result<String, CliError> {
    let options = CompareMetricsOptions {
        project_path: last_value(args.project.path),
        flow_id: Some(args.flow_id),
        baseline_step: last_value(args.baseline),
        candidate_step: last_value(args.candidate),
        metric: last_value(args.metric),
        direction: last_value(args.direction),
        json: args.project.json,
    };
    let flow_id = options.flow_id.expect("clap requires flow id");
    let baseline_step = options.baseline_step.ok_or_else(|| {
        CliError::InvalidArgument("compare metrics requires --baseline".to_string())
    })?;
    let candidate_step = options.candidate_step.ok_or_else(|| {
        CliError::InvalidArgument("compare metrics requires --candidate".to_string())
    })?;
    let metric = options.metric.ok_or_else(|| {
        CliError::InvalidArgument("compare metrics requires --metric".to_string())
    })?;
    let project_path = options.project_path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let result =
        store.compare_observed_metric(agentflow_core::comparison::MetricComparisonRequest {
            flow_id,
            baseline_step,
            candidate_step,
            metric,
            direction: options.direction.unwrap_or_else(|| "higher".to_string()),
        })?;

    if options.json {
        Ok(result.to_json())
    } else {
        Ok(format!(
            "Recorded metric comparison\nId: {}\nMetric: {}\nDirection: {}\nBaseline: {}\nCandidate: {}\nWinner: {}",
            result.comparison.id,
            result.metric,
            result.direction,
            result.baseline_value,
            result.candidate_value,
            result.comparison.winner.as_deref().unwrap_or("none")
        ))
    }
}

fn compare_list_command(args: CompareListArgs) -> Result<String, CliError> {
    let flow_id = args.flow_id;
    let options = ProjectOptions::from(args.project);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let comparisons = store.list_branch_comparisons(&flow_id)?;

    if options.json {
        Ok(branch_comparisons_json(&flow_id, &comparisons))
    } else if comparisons.is_empty() {
        Ok(format!("No branch comparisons recorded for flow {flow_id}"))
    } else {
        Ok(comparisons
            .iter()
            .map(|comparison| format_branch_comparison("Branch comparison", comparison))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn compare_inspect_command(args: CompareInspectArgs) -> Result<String, CliError> {
    let comparison_id = args.comparison_id;
    let options = ProjectOptions::from(args.project);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let comparison = store.inspect_branch_comparison(&comparison_id)?;

    if options.json {
        Ok(comparison.to_json())
    } else {
        Ok(format_branch_comparison("Branch comparison", &comparison))
    }
}

fn doctor_command(args: PathJsonArgs) -> Result<String, CliError> {
    let options = ProjectOptions::from(args);
    let path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let migrations = store.applied_migrations()?;
    Ok(format!(
        "AgentFlow project ok\nPath: {}\nApplied migrations: {}",
        store.root_path().display(),
        migrations.len()
    ))
}

fn tools_command(args: ToolsArgs) -> Result<String, CliError> {
    match args.command {
        ToolsCommand::Register(args) => tools_register_command(args),
        ToolsCommand::List(args) => tools_list_command(args),
        ToolsCommand::Inspect(args) => tools_inspect_command(args),
        ToolsCommand::Match(args) => tools_match_command(args),
        ToolsCommand::DraftStep(args) => tools_draft_step_command(args),
    }
}

fn tools_register_command(args: ToolsRegisterArgs) -> Result<String, CliError> {
    let spec_path = args.tool_yaml;
    let project_path = project_path_from_only(args.project)?;
    let source = fs::read_to_string(&spec_path)?;
    let spec = agentflow_core::storage::ToolSpec::from_simple_yaml(&source)?;
    let spec = resolve_tool_runtime_paths(spec, Path::new(&spec_path))?;
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let registration = store.register_tool(spec)?;
    let action = if registration.replaced_existing_version {
        "Updated"
    } else {
        "Registered"
    };

    Ok(format!(
        "{action} tool\nRef: {}\nVersion: {}\nSpec hash: {}",
        registration.tool_ref, registration.version, registration.spec_hash
    ))
}

fn resolve_tool_runtime_paths(
    mut spec: agentflow_core::storage::ToolSpec,
    spec_path: &Path,
) -> Result<agentflow_core::storage::ToolSpec, CliError> {
    let Some(spec_dir) = spec_path.parent() else {
        return Ok(spec);
    };

    for arg in spec.runtime.command.iter_mut().skip(1) {
        if arg.starts_with('-') {
            continue;
        }
        let path = Path::new(arg);
        if path.is_absolute() {
            continue;
        }
        let candidate = spec_dir.join(path);
        if candidate.exists() {
            *arg = fs::canonicalize(candidate)?.display().to_string();
        }
    }
    resolve_runtime_path_field(&mut spec.runtime.env_file, spec_dir)?;
    resolve_runtime_path_field(&mut spec.runtime.env_prefix, spec_dir)?;

    Ok(spec)
}

fn resolve_runtime_path_field(value: &mut Option<String>, spec_dir: &Path) -> Result<(), CliError> {
    let Some(current) = value.as_deref() else {
        return Ok(());
    };
    let path = Path::new(current);
    if path.is_absolute() {
        return Ok(());
    }
    let candidate = spec_dir.join(path);
    if candidate.exists() {
        *value = Some(fs::canonicalize(candidate)?.display().to_string());
    }
    Ok(())
}

fn tools_list_command(args: PathJsonArgs) -> Result<String, CliError> {
    let options = ProjectOptions::from(args);
    let path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let tools = store.list_tools()?;

    if options.json {
        Ok(tools_list_json(&tools))
    } else if tools.is_empty() {
        Ok("No tools registered".to_string())
    } else {
        Ok(tools
            .iter()
            .map(|tool| {
                format!(
                    "{}@{} [{}]",
                    tool.tool_ref(),
                    tool.latest_version,
                    tool.maturity
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn tools_inspect_command(args: ToolsInspectArgs) -> Result<String, CliError> {
    let tool_ref = args.tool_ref;
    let options = ProjectOptions::from(args.project);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let inspection = store.inspect_tool(&tool_ref)?;

    if options.json {
        Ok(inspection.to_json())
    } else {
        Ok(format!(
            "Tool: {}\nLatest version: {}\nSelected version: {}\nMaturity: {}\nSpec hash: {}\nCreated: {}\nStored spec JSON:\n{}",
            inspection.summary.tool_ref(),
            inspection.summary.latest_version,
            inspection.version,
            inspection.summary.maturity,
            inspection.spec_hash,
            inspection.created_at,
            inspection.spec_json
        ))
    }
}

fn tools_match_command(args: ToolsMatchArgs) -> Result<String, CliError> {
    let options = ToolsMatchOptions::try_from(args)?;
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let candidates = store.match_tools(&agentflow_core::tool_select::CapabilityQuery {
        desired_output_type: options.output,
        available_input_types: options.inputs,
        keywords: options.keywords,
    })?;

    if options.project.json {
        Ok(tool_candidates_json(&candidates))
    } else {
        Ok(format_tool_candidates(&candidates))
    }
}

fn tools_draft_step_command(args: ToolsDraftStepArgs) -> Result<String, CliError> {
    let options = ToolsDraftStepOptions::try_from(args)?;
    let tool_ref = options.tool_ref.expect("clap requires tool ref");
    let path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let mut step = store.draft_step_for(&tool_ref, &options.inputs)?;
    if options.infer_params {
        let hypothesis_id = options.hypothesis_id.as_deref().ok_or_else(|| {
            CliError::InvalidArgument("--infer-params requires --hypothesis <id>".to_string())
        })?;
        let hypothesis = store.inspect_hypothesis(hypothesis_id)?;
        let synthesizer = synth_commands::configured_or_default_synthesizer(
            store.root_path(),
            options.synthesizer,
        )?;
        infer_draft_step_params(
            store.root_path(),
            &mut step,
            &hypothesis.statement,
            &synthesizer,
        );
    }

    if options.project.json {
        Ok(proposed_step_json(&step))
    } else {
        Ok(format_proposed_step(&step))
    }
}

fn infer_draft_step_params(
    project_root: &Path,
    step: &mut agentflow_core::branch::ProposedStep,
    statement: &str,
    synthesizer: &str,
) {
    for (param_name, param_value) in &mut step.params {
        if param_value != &format!("REPLACE_{param_name}") {
            continue;
        }

        let prompt = format!(
            "Research hypothesis: \"{statement}\". A bioinformatics analysis tool needs a value for the parameter \"{param_name}\". Reply with ONLY the value (e.g. a gene symbol like THRSP), no explanation, no quotes."
        );
        let Ok(candidate) =
            synth_commands::run_project_synthesizer(project_root, synthesizer, &prompt)
        else {
            continue;
        };
        let stripped = synth_commands::strip_markdown_fence(&candidate);
        let Some(inferred) = first_non_empty_line(&stripped) else {
            continue;
        };
        *param_value = inferred.to_string();
    }
}

fn first_non_empty_line(value: &str) -> Option<&str> {
    value.lines().map(str::trim).find(|line| !line.is_empty())
}

fn env_command(args: EnvArgs) -> Result<String, CliError> {
    match args.command {
        EnvCommand::Check(args) => env_check_command(args),
        EnvCommand::Prepare(args) => env_prepare_command(args),
        EnvCommand::Export(args) => env_export_command(args),
    }
}

fn env_check_command(args: ToolRefJsonArgs) -> Result<String, CliError> {
    let tool_ref = args.tool_ref;
    let options = ProjectOptions::from(args.project);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let check = store.check_tool_environment(&tool_ref)?;
    if options.json {
        Ok(environment_check_json(&check))
    } else {
        Ok(format_environment_check(&check))
    }
}

fn env_prepare_command(args: ToolRefJsonArgs) -> Result<String, CliError> {
    let tool_ref = args.tool_ref;
    let options = ProjectOptions::from(args.project);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let prepare = store.prepare_tool_environment(&tool_ref)?;
    if options.json {
        Ok(environment_prepare_json(&prepare))
    } else {
        Ok(format_environment_prepare(&prepare))
    }
}

fn env_export_command(args: ToolRefJsonArgs) -> Result<String, CliError> {
    let tool_ref = args.tool_ref;
    let options = ProjectOptions::from(args.project);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let export = store.export_tool_environment(&tool_ref)?;
    if options.json {
        Ok(environment_export_json(&export))
    } else {
        Ok(format_environment_export(&export))
    }
}

fn import_command(args: ImportArgs) -> Result<String, CliError> {
    let options = ImportOptions::try_from(args)?;
    let source_path = options.source_path.expect("clap requires import file");
    let artifact_type = options.artifact_type.ok_or_else(|| {
        CliError::InvalidArgument("import requires --type <artifact-type>".to_string())
    })?;
    let project_path = options.project_path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let imported = store.import_artifact(agentflow_core::storage::ArtifactImportRequest {
        source_path: PathBuf::from(source_path),
        artifact_type,
        mode: options.mode,
    })?;

    Ok(format!(
        "Imported artifact\nId: {}\nType: {}\nMode: {}\nPath: {}\nHash: {}",
        imported.summary.id,
        imported.summary.artifact_type,
        options.mode,
        imported.summary.path.display(),
        imported.summary.hash.unwrap_or_else(|| "none".to_string())
    ))
}

fn artifacts_command(args: ArtifactsArgs) -> Result<String, CliError> {
    match args.command {
        ArtifactsCommand::List(args) => artifacts_list_command(args),
        ArtifactsCommand::Inspect(args) => artifacts_inspect_command(args),
    }
}

fn artifacts_list_command(args: PathJsonArgs) -> Result<String, CliError> {
    let options = ProjectOptions::from(args);
    let path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let artifacts = store.list_artifacts()?;

    if options.json {
        Ok(agentflow_core::storage::artifacts_list_json(&artifacts))
    } else if artifacts.is_empty() {
        Ok("No artifacts registered".to_string())
    } else {
        Ok(artifacts
            .iter()
            .map(|artifact| {
                format!(
                    "{} [{}:{}] {}",
                    artifact.id,
                    artifact.kind,
                    artifact.artifact_type,
                    artifact.path.display()
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn artifacts_inspect_command(args: ArtifactInspectArgs) -> Result<String, CliError> {
    let artifact_id = args.artifact_id;
    let options = ProjectOptions::from(args.project);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let inspection = store.inspect_artifact(&artifact_id)?;

    if options.json {
        Ok(inspection.to_json())
    } else {
        Ok(format!(
            "Artifact: {}\nKind: {}\nType: {}\nPath: {}\nHash: {}\nSize: {}\nCreated: {}\nValidation:\n{}",
            inspection.summary.id,
            inspection.summary.kind,
            inspection.summary.artifact_type,
            inspection.summary.path.display(),
            inspection.summary.hash.unwrap_or_else(|| "none".to_string()),
            inspection
                .summary
                .size_bytes
                .map_or_else(|| "unknown".to_string(), |size| size.to_string()),
            inspection.summary.created_at,
            inspection.validation_json
        ))
    }
}

fn flow_command(args: FlowArgs) -> Result<String, CliError> {
    match args.command {
        FlowCommand::Validate(args) => flow_validate_command(args),
        FlowCommand::Approve(args) => flow_approve_command(args),
        FlowCommand::Inspect(args) => flow_inspect_command(args),
    }
}

fn flow_validate_command(args: FlowValidateArgs) -> Result<String, CliError> {
    let flow_path = args.flow_yaml;
    let options = ProjectOptions::from(args.project);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let source = fs::read_to_string(&flow_path)?;
    let draft = agentflow_core::storage::FlowDraft::from_simple_yaml(&source)?;
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let report = store.validate_flow(&draft);

    if options.json {
        Ok(report.to_json())
    } else if report.valid {
        Ok(format!(
            "Flow is valid\nId: {}\nName: {}\nSteps: {}\nEdges: {}",
            report.flow_id, report.name, report.step_count, report.edge_count
        ))
    } else {
        Err(CliError::Core(format!(
            "flow validation failed: {}",
            report
                .issues
                .iter()
                .map(|issue| issue.message.clone())
                .collect::<Vec<_>>()
                .join("; ")
        )))
    }
}

fn flow_approve_command(args: FlowApproveArgs) -> Result<String, CliError> {
    let flow_path = args.flow_yaml;
    let project_path = project_path_from_only(args.project)?;
    let source = fs::read_to_string(&flow_path)?;
    let draft = agentflow_core::storage::FlowDraft::from_simple_yaml(&source)?;
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let approval = store.approve_flow(draft, Some(PathBuf::from(&flow_path).as_path()))?;

    Ok(format!(
        "Approved flow\nId: {}\nName: {}\nSteps: {}\nEdges: {}",
        approval.flow_id, approval.name, approval.step_count, approval.edge_count
    ))
}

fn flow_inspect_command(args: FlowInspectArgs) -> Result<String, CliError> {
    let flow_id = args.flow_id;
    let options = ProjectOptions::from(args.project);
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let inspection = store.inspect_flow(&flow_id)?;

    if options.json {
        Ok(inspection.to_json())
    } else {
        Ok(format!(
            "Flow: {}\nName: {}\nStatus: {}\nSteps: {}\nEdges: {}",
            inspection.id,
            inspection.name,
            inspection.status,
            inspection.steps.len(),
            inspection.edges.len()
        ))
    }
}

#[derive(Debug, Default)]
struct ProjectOptions {
    name: Option<String>,
    path: Option<PathBuf>,
    json: bool,
}

#[derive(Debug, Default)]
struct ToolsMatchOptions {
    project: ProjectOptions,
    output: Option<String>,
    inputs: Vec<String>,
    keywords: Vec<String>,
}

#[derive(Debug, Default)]
struct ToolsDraftStepOptions {
    project: ProjectOptions,
    tool_ref: Option<String>,
    inputs: Vec<(String, String)>,
    hypothesis_id: Option<String>,
    infer_params: bool,
    synthesizer: Option<String>,
}

#[derive(Debug, Default)]
struct CachePruneOptions {
    project: ProjectOptions,
    all: bool,
    older_than_seconds: Option<u64>,
}

#[derive(Debug, Default)]
struct RunsListOptions {
    project: ProjectOptions,
    flow_id: Option<String>,
}

#[derive(Debug)]
struct ImportOptions {
    project_path: Option<PathBuf>,
    source_path: Option<String>,
    artifact_type: Option<String>,
    mode: agentflow_core::storage::ArtifactImportMode,
}

#[derive(Debug, Default)]
struct ObserveOptions {
    project_path: Option<PathBuf>,
    artifact_id: Option<String>,
    adapter: Option<String>,
    json: bool,
}

#[derive(Debug, Default)]
struct ResearchNoteOptions {
    project_path: Option<PathBuf>,
    problem: Option<String>,
    question: Option<String>,
    finding: Option<String>,
    confidence: Option<String>,
    source: Option<String>,
}

#[derive(Debug, Default)]
struct GraphPatchProposeOptions {
    project_path: Option<PathBuf>,
    flow_id: Option<String>,
    title: Option<String>,
    reason: Option<String>,
    patch_json: Option<String>,
    patch_file: Option<PathBuf>,
    json: bool,
}

#[derive(Debug, Default)]
struct GraphPatchRejectOptions {
    project_path: Option<PathBuf>,
    patch_id: Option<String>,
    reason: Option<String>,
    json: bool,
}

#[derive(Debug, Default)]
struct CompareStepsOptions {
    project_path: Option<PathBuf>,
    flow_id: Option<String>,
    baseline_step: Option<String>,
    candidate_step: Option<String>,
    summary: Option<String>,
    winner: Option<String>,
    reason: Option<String>,
    json: bool,
}

#[derive(Debug, Default)]
struct CompareMetricsOptions {
    project_path: Option<PathBuf>,
    flow_id: Option<String>,
    baseline_step: Option<String>,
    candidate_step: Option<String>,
    metric: Option<String>,
    direction: Option<String>,
    json: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            project_path: None,
            source_path: None,
            artifact_type: None,
            mode: agentflow_core::storage::ArtifactImportMode::Reference,
        }
    }
}

pub(crate) fn last_value<T>(mut values: Vec<T>) -> Option<T> {
    values.pop()
}

pub(crate) fn project_path_from_only(args: PathOnlyArgs) -> Result<PathBuf, CliError> {
    Ok(last_value(args.path).unwrap_or(std::env::current_dir()?))
}

pub(crate) fn project_path_from_json(args: PathJsonArgs) -> Result<PathBuf, CliError> {
    Ok(last_value(args.path).unwrap_or(std::env::current_dir()?))
}

pub(crate) fn parse_usize_value(flag: &str, value: &str) -> Result<usize, CliError> {
    value
        .parse::<usize>()
        .map_err(|_| CliError::InvalidArgument(format!("{flag} must be a non-negative integer")))
}

pub(crate) fn parse_u32_value(flag: &str, value: &str) -> Result<u32, CliError> {
    let value = parse_usize_value(flag, value)?;
    u32::try_from(value).map_err(|_| {
        CliError::InvalidArgument(format!("{flag} must fit in an unsigned 32-bit integer"))
    })
}

impl From<PathOnlyArgs> for ProjectOptions {
    fn from(args: PathOnlyArgs) -> Self {
        Self {
            name: None,
            path: last_value(args.path),
            json: false,
        }
    }
}

impl From<PathJsonArgs> for ProjectOptions {
    fn from(args: PathJsonArgs) -> Self {
        Self {
            name: None,
            path: last_value(args.path),
            json: args.json,
        }
    }
}

impl From<InitArgs> for ProjectOptions {
    fn from(args: InitArgs) -> Self {
        Self {
            name: last_value(args.name),
            path: last_value(args.project.path),
            json: args.project.json,
        }
    }
}

impl TryFrom<ImportArgs> for ImportOptions {
    type Error = CliError;

    fn try_from(args: ImportArgs) -> Result<Self, Self::Error> {
        let mode = match last_value(args.mode) {
            Some(mode) => {
                agentflow_core::storage::ArtifactImportMode::parse(&mode).ok_or_else(|| {
                    CliError::InvalidArgument("--mode must be reference or copy".to_string())
                })?
            }
            None => agentflow_core::storage::ArtifactImportMode::Reference,
        };

        Ok(Self {
            project_path: last_value(args.project.path),
            source_path: Some(args.file.display().to_string()),
            artifact_type: last_value(args.artifact_type),
            mode,
        })
    }
}

impl TryFrom<ToolsMatchArgs> for ToolsMatchOptions {
    type Error = CliError;

    fn try_from(args: ToolsMatchArgs) -> Result<Self, Self::Error> {
        let output = match args.output.len() {
            0 => None,
            1 => last_value(args.output),
            _ => {
                return Err(CliError::InvalidArgument(
                    "--output may only be provided once".to_string(),
                ));
            }
        };

        Ok(Self {
            project: ProjectOptions::from(args.project),
            output,
            inputs: args.input,
            keywords: args.keyword,
        })
    }
}

impl TryFrom<ToolsDraftStepArgs> for ToolsDraftStepOptions {
    type Error = CliError;

    fn try_from(args: ToolsDraftStepArgs) -> Result<Self, Self::Error> {
        let inputs = args
            .input
            .iter()
            .map(|input| parse_draft_step_input(input))
            .collect::<Result<Vec<_>, _>>()?;
        let options = Self {
            project: ProjectOptions::from(args.project),
            tool_ref: Some(args.tool_ref),
            inputs,
            hypothesis_id: last_value(args.hypothesis),
            infer_params: args.infer_params,
            synthesizer: last_value(args.synthesizer),
        };

        if options.infer_params && options.hypothesis_id.is_none() {
            return Err(CliError::InvalidArgument(
                "--infer-params requires --hypothesis <id>".to_string(),
            ));
        }

        Ok(options)
    }
}

fn parse_draft_step_input(value: &str) -> Result<(String, String), CliError> {
    let Some((type_name, artifact_id)) = value.split_once(':') else {
        return Err(CliError::InvalidArgument(
            "--input must use <type>:<artifact-id>".to_string(),
        ));
    };
    if type_name.trim().is_empty() || artifact_id.trim().is_empty() {
        return Err(CliError::InvalidArgument(
            "--input must use non-empty <type>:<artifact-id>".to_string(),
        ));
    }
    Ok((type_name.to_string(), artifact_id.to_string()))
}

impl TryFrom<CachePruneArgs> for CachePruneOptions {
    type Error = CliError;

    fn try_from(args: CachePruneArgs) -> Result<Self, Self::Error> {
        let older_than_seconds = last_value(args.older_than_seconds)
            .map(|value| {
                let seconds = value.parse::<u64>().map_err(|_| {
                    CliError::InvalidArgument(
                        "--older-than-seconds must be a positive integer".to_string(),
                    )
                })?;
                if seconds == 0 {
                    return Err(CliError::InvalidArgument(
                        "--older-than-seconds must be greater than zero".to_string(),
                    ));
                }
                Ok(seconds)
            })
            .transpose()?;

        let options = Self {
            project: ProjectOptions::from(args.project),
            all: args.all,
            older_than_seconds,
        };

        if options.all && options.older_than_seconds.is_some() {
            return Err(CliError::InvalidArgument(
                "use either cache prune --all or --older-than-seconds, not both".to_string(),
            ));
        }
        if !options.all && options.older_than_seconds.is_none() {
            return Err(CliError::InvalidArgument(
                "cache prune requires --all or --older-than-seconds <seconds>".to_string(),
            ));
        }

        Ok(options)
    }
}

fn format_observation(observation: &agentflow_core::ObservationRecord) -> String {
    format!(
        "Observation: {}\nArtifact: {}\nKind: {}\nSeverity: {}\nSummary: {}\nCreated: {}",
        observation.id,
        observation.artifact_id.as_deref().unwrap_or("none"),
        observation.kind,
        observation.severity,
        observation.summary,
        observation.created_at
    )
}

fn observation_json(observation: &agentflow_core::ObservationRecord) -> String {
    format!(
        concat!(
            "{{",
            "\"id\":\"{}\",",
            "\"flow_id\":{},",
            "\"step_id\":{},",
            "\"artifact_id\":{},",
            "\"kind\":\"{}\",",
            "\"severity\":\"{}\",",
            "\"summary\":\"{}\",",
            "\"payload\":{},",
            "\"created_at\":{}",
            "}}"
        ),
        escape_json(&observation.id),
        optional_json_string(observation.flow_id.as_deref()),
        optional_json_string(observation.step_id.as_deref()),
        optional_json_string(observation.artifact_id.as_deref()),
        escape_json(&observation.kind),
        escape_json(&observation.severity),
        escape_json(&observation.summary),
        observation.payload_json,
        observation.created_at
    )
}

fn observations_json(observations: &[agentflow_core::ObservationRecord]) -> String {
    let items = observations
        .iter()
        .map(observation_json)
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"schema_version\":\"agentflow.observation_list.v0\",\"observations\":[{items}]}}")
}

fn format_graph_patch(
    heading: &str,
    patch: &agentflow_core::graph_patch::GraphPatchRecord,
) -> String {
    format!(
        "{heading}\nId: {}\nFlow: {}\nStatus: {}\nTitle: {}\nReason: {}\nDecision reason: {}",
        patch.id,
        patch.flow_id,
        patch.status,
        patch.title,
        patch.reason,
        patch.decision_reason.as_deref().unwrap_or("none")
    )
}

fn graph_patch_json(patch: &agentflow_core::graph_patch::GraphPatchRecord) -> String {
    format!(
        concat!(
            "{{",
            "\"id\":\"{}\",",
            "\"flow_id\":\"{}\",",
            "\"title\":\"{}\",",
            "\"reason\":\"{}\",",
            "\"patch_json\":\"{}\",",
            "\"status\":\"{}\",",
            "\"decision_reason\":{},",
            "\"created_at\":{},",
            "\"decided_at\":{}",
            "}}"
        ),
        escape_json(&patch.id),
        escape_json(&patch.flow_id),
        escape_json(&patch.title),
        escape_json(&patch.reason),
        escape_json(&patch.patch_json),
        escape_json(&patch.status),
        optional_json_string(patch.decision_reason.as_deref()),
        patch.created_at,
        optional_json_i64(patch.decided_at)
    )
}

fn graph_patches_json(
    flow_id: &str,
    patches: &[agentflow_core::graph_patch::GraphPatchRecord],
) -> String {
    let items = patches
        .iter()
        .map(graph_patch_json)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"schema_version\":\"agentflow.graph_patch_list.v0\",\"flow_id\":\"{}\",\"patches\":[{}]}}",
        escape_json(flow_id),
        items
    )
}

fn graph_patch_application_json(
    application: &agentflow_core::graph_patch::GraphPatchApplication,
) -> String {
    let steps = application
        .applied_steps
        .iter()
        .map(|step| format!("\"{}\"", escape_json(step)))
        .collect::<Vec<_>>()
        .join(",");
    let edges = application
        .applied_edges
        .iter()
        .map(|(from, to)| {
            format!(
                "{{\"from\":\"{}\",\"to\":\"{}\"}}",
                escape_json(from),
                escape_json(to)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let updated_steps = application
        .updated_steps
        .iter()
        .map(|step| format!("\"{}\"", escape_json(step)))
        .collect::<Vec<_>>()
        .join(",");
    let invalidated_steps = application
        .invalidated_steps
        .iter()
        .map(|step| format!("\"{}\"", escape_json(step)))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"schema_version\":\"agentflow.graph_patch_application.v0\",\"patch_id\":\"{}\",\"flow_id\":\"{}\",\"applied_steps\":[{}],\"applied_edges\":[{}],\"updated_steps\":[{}],\"invalidated_steps\":[{}]}}",
        escape_json(&application.patch_id),
        escape_json(&application.flow_id),
        steps,
        edges,
        updated_steps,
        invalidated_steps
    )
}

fn format_branch_comparison(
    heading: &str,
    comparison: &agentflow_core::comparison::BranchComparisonRecord,
) -> String {
    format!(
        "{heading}\nId: {}\nFlow: {}\nBaseline: {}\nCandidate: {}\nWinner: {}\nSummary: {}\nReason: {}",
        comparison.id,
        comparison.flow_id,
        comparison.baseline_step,
        comparison.candidate_step,
        comparison.winner.as_deref().unwrap_or("none"),
        comparison.summary,
        comparison.reason.as_deref().unwrap_or("none")
    )
}

fn branch_comparisons_json(
    flow_id: &str,
    comparisons: &[agentflow_core::comparison::BranchComparisonRecord],
) -> String {
    let items = comparisons
        .iter()
        .map(agentflow_core::comparison::BranchComparisonRecord::to_json)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"schema_version\":\"agentflow.branch_comparison_list.v0\",\"flow_id\":\"{}\",\"comparisons\":[{}]}}",
        escape_json(flow_id),
        items
    )
}

fn tools_list_json(tools: &[agentflow_core::storage::ToolSummary]) -> String {
    let items = tools
        .iter()
        .map(|tool| {
            format!(
                concat!(
                    "{{",
                    "\"ref\":\"{}\",",
                    "\"namespace\":\"{}\",",
                    "\"name\":\"{}\",",
                    "\"latest_version\":\"{}\",",
                    "\"maturity\":\"{}\"",
                    "}}"
                ),
                escape_json(&tool.tool_ref()),
                escape_json(&tool.namespace),
                escape_json(&tool.name),
                escape_json(&tool.latest_version),
                escape_json(&tool.maturity)
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    format!(
        "{{\"schema_version\":\"{}\",\"tools\":[{}]}}",
        agentflow_schemas::TOOL_LIST_JSON_SCHEMA_V0,
        items
    )
}

fn format_tool_candidates(candidates: &[agentflow_core::tool_select::ToolCandidate]) -> String {
    if candidates.is_empty() {
        return "No matching tools".to_string();
    }

    candidates
        .iter()
        .map(|candidate| {
            format!(
                "{} [{}] score={} reason={}",
                candidate.tool_ref,
                candidate.fit.as_str(),
                candidate.score,
                candidate.reason
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_candidates_json(candidates: &[agentflow_core::tool_select::ToolCandidate]) -> String {
    let items = candidates
        .iter()
        .map(|candidate| {
            format!(
                concat!(
                    "{{",
                    "\"tool_ref\":\"{}\",",
                    "\"fit\":\"{}\",",
                    "\"score\":{},",
                    "\"reason\":\"{}\"",
                    "}}"
                ),
                escape_json(&candidate.tool_ref),
                candidate.fit.as_str(),
                candidate.score,
                escape_json(&candidate.reason)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
}

fn format_proposed_step(step: &agentflow_core::branch::ProposedStep) -> String {
    format!(
        "Step: {}\nTool: {}\nNeeds: {}\nInputs:\n{}\nParams:\n{}\nOutputs:\n{}",
        step.id,
        step.tool,
        format_string_list(&step.needs),
        format_pairs(&step.inputs),
        format_pairs(&step.params),
        format_pairs(&step.outputs)
    )
}

fn format_pairs(pairs: &[(String, String)]) -> String {
    if pairs.is_empty() {
        return "  none".to_string();
    }

    pairs
        .iter()
        .map(|(key, value)| format!("  {key}: {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn proposed_step_json(step: &agentflow_core::branch::ProposedStep) -> String {
    format!(
        concat!(
            "{{",
            "\"id\":\"{}\",",
            "\"tool\":\"{}\",",
            "\"needs\":{},",
            "\"inputs\":{},",
            "\"params\":{},",
            "\"outputs\":{}",
            "}}"
        ),
        escape_json(&step.id),
        escape_json(&step.tool),
        string_list_json(&step.needs),
        pairs_object_json(&step.inputs),
        pairs_object_json(&step.params),
        pairs_object_json(&step.outputs)
    )
}

fn string_list_json(values: &[String]) -> String {
    let items = values
        .iter()
        .map(|value| format!("\"{}\"", escape_json(value)))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
}

fn pairs_object_json(pairs: &[(String, String)]) -> String {
    let fields = pairs
        .iter()
        .map(|(key, value)| format!("\"{}\":\"{}\"", escape_json(key), escape_json(value)))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{fields}}}")
}

fn research_notes_json(notes: &[agentflow_core::research::ResearchNote]) -> String {
    let items = notes
        .iter()
        .map(agentflow_core::research::ResearchNote::to_json)
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"schema_version\":\"agentflow.research_notes.v0\",\"notes\":[{items}]}}")
}

fn environment_check_json(check: &agentflow_core::runtime::EnvironmentCheckSummary) -> String {
    let items = check
        .items
        .iter()
        .map(environment_check_item_json)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        concat!(
            "{{",
            "\"schema_version\":\"agentflow.env_check.v0\",",
            "\"tool_ref\":\"{}\",",
            "\"version\":\"{}\",",
            "\"backend\":\"{}\",",
            "\"ok\":{},",
            "\"items\":[{}]",
            "}}"
        ),
        escape_json(&check.tool_ref),
        escape_json(&check.version),
        escape_json(&check.backend),
        check.ok,
        items
    )
}

fn environment_check_item_json(item: &agentflow_core::runtime::EnvironmentCheckItem) -> String {
    format!(
        concat!(
            "{{",
            "\"name\":\"{}\",",
            "\"status\":\"{}\",",
            "\"message\":\"{}\",",
            "\"details\":{}",
            "}}"
        ),
        escape_json(&item.name),
        escape_json(&item.status),
        escape_json(&item.message),
        optional_json_string(item.details.as_deref())
    )
}

fn environment_prepare_json(
    prepare: &agentflow_core::runtime::EnvironmentPrepareSummary,
) -> String {
    let command = json_string_array(&prepare.command);
    let items = prepare
        .items
        .iter()
        .map(environment_check_item_json)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        concat!(
            "{{",
            "\"schema_version\":\"agentflow.env_prepare.v0\",",
            "\"tool_ref\":\"{}\",",
            "\"version\":\"{}\",",
            "\"backend\":\"{}\",",
            "\"ok\":{},",
            "\"status\":\"{}\",",
            "\"command\":{},",
            "\"exit_code\":{},",
            "\"stdout\":\"{}\",",
            "\"stderr\":\"{}\",",
            "\"items\":[{}]",
            "}}"
        ),
        escape_json(&prepare.tool_ref),
        escape_json(&prepare.version),
        escape_json(&prepare.backend),
        prepare.ok,
        escape_json(&prepare.status),
        command,
        optional_json_i32(prepare.exit_code),
        escape_json(&prepare.stdout),
        escape_json(&prepare.stderr),
        items
    )
}

fn environment_export_json(export: &agentflow_core::runtime::EnvironmentExportSummary) -> String {
    let command = json_string_array(&export.command);
    let declared_packages = json_string_array(&export.declared_packages);
    let exported_packages = json_string_array(&export.exported_packages);
    let missing_packages = json_string_array(&export.missing_packages);
    let extra_packages = json_string_array(&export.extra_packages);
    let items = export
        .items
        .iter()
        .map(environment_check_item_json)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        concat!(
            "{{",
            "\"schema_version\":\"agentflow.env_export.v0\",",
            "\"tool_ref\":\"{}\",",
            "\"version\":\"{}\",",
            "\"backend\":\"{}\",",
            "\"ok\":{},",
            "\"status\":\"{}\",",
            "\"command\":{},",
            "\"exit_code\":{},",
            "\"export_hash\":{},",
            "\"declared_packages\":{},",
            "\"exported_packages\":{},",
            "\"missing_packages\":{},",
            "\"extra_packages\":{},",
            "\"stdout\":\"{}\",",
            "\"stderr\":\"{}\",",
            "\"items\":[{}]",
            "}}"
        ),
        escape_json(&export.tool_ref),
        escape_json(&export.version),
        escape_json(&export.backend),
        export.ok,
        escape_json(&export.status),
        command,
        optional_json_i32(export.exit_code),
        optional_json_string(export.export_hash.as_deref()),
        declared_packages,
        exported_packages,
        missing_packages,
        extra_packages,
        escape_json(&export.stdout),
        escape_json(&export.stderr),
        items
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

fn optional_json_string(value: Option<&str>) -> String {
    value.map_or_else(
        || "null".to_string(),
        |value| format!("\"{}\"", escape_json(value)),
    )
}

fn optional_json_i64(value: Option<i64>) -> String {
    value.map_or_else(|| "null".to_string(), |value| value.to_string())
}

fn optional_json_i32(value: Option<i32>) -> String {
    value.map_or_else(|| "null".to_string(), |value| value.to_string())
}

fn optional_json_path(value: Option<&PathBuf>) -> String {
    value.map_or_else(
        || "null".to_string(),
        |value| format!("\"{}\"", escape_json(&value.display().to_string())),
    )
}

fn cache_entries_json(entries: &[agentflow_core::runtime::CacheEntrySummary]) -> String {
    let entries = entries
        .iter()
        .map(|entry| {
            format!(
                "{{\"cache_key\":\"{}\",\"tool_ref\":\"{}\",\"output_count\":{},\"created_at\":{},\"last_used_at\":{}}}",
                escape_json(&entry.cache_key),
                escape_json(&entry.tool_ref),
                entry.output_count,
                entry.created_at,
                entry.last_used_at
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"schema_version\":\"agentflow.cache_entries.v0\",\"entries\":[{entries}]}}")
}

fn runs_json(runs: &[agentflow_core::runtime::RunRecordSummary]) -> String {
    let runs = runs
        .iter()
        .map(run_summary_json)
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"schema_version\":\"agentflow.runs.v0\",\"runs\":[{runs}]}}")
}

fn run_inspection_json(inspection: &agentflow_core::runtime::RunInspection) -> String {
    let attempts = inspection
        .attempts
        .iter()
        .map(run_attempt_record_json)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"schema_version\":\"agentflow.run_inspection.v0\",\"run\":{},\"attempts\":[{}]}}",
        run_summary_json(&inspection.run),
        attempts
    )
}

fn run_summary_json(run: &agentflow_core::runtime::RunRecordSummary) -> String {
    format!(
        "{{\"run_id\":\"{}\",\"flow_id\":\"{}\",\"step_id\":\"{}\",\"status\":\"{}\",\"attempt_count\":{},\"latest_attempt_id\":{},\"cache_key\":{},\"created_at\":{},\"updated_at\":{}}}",
        escape_json(&run.run_id),
        escape_json(&run.flow_id),
        escape_json(&run.step_id),
        escape_json(&run.status),
        run.attempt_count,
        optional_json_string(run.latest_attempt_id.as_deref()),
        optional_json_string(run.cache_key.as_deref()),
        run.created_at,
        run.updated_at
    )
}

fn run_attempt_record_json(attempt: &agentflow_core::runtime::RunAttemptRecord) -> String {
    format!(
        "{{\"attempt_id\":\"{}\",\"run_id\":\"{}\",\"attempt\":{},\"status\":\"{}\",\"workdir\":{},\"started_at\":{},\"ended_at\":{},\"exit_code\":{},\"stdout_path\":{},\"stderr_path\":{},\"error_class\":{},\"error_message\":{}}}",
        escape_json(&attempt.attempt_id),
        escape_json(&attempt.run_id),
        attempt.attempt,
        escape_json(&attempt.status),
        optional_json_path(attempt.workdir.as_ref()),
        optional_json_i64(attempt.started_at),
        optional_json_i64(attempt.ended_at),
        optional_json_i32(attempt.exit_code),
        optional_json_path(attempt.stdout_path.as_ref()),
        optional_json_path(attempt.stderr_path.as_ref()),
        optional_json_string(attempt.error_class.as_deref()),
        optional_json_string(attempt.error_message.as_deref())
    )
}

fn format_runs(runs: &[agentflow_core::runtime::RunRecordSummary]) -> String {
    runs.iter()
        .map(|run| {
            format!(
                "{} {} {} [{}] attempts:{} latest:{}",
                run.run_id,
                run.flow_id,
                run.step_id,
                run.status,
                run.attempt_count,
                run.latest_attempt_id.as_deref().unwrap_or("none")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_run_attempt_records(attempts: &[agentflow_core::runtime::RunAttemptRecord]) -> String {
    if attempts.is_empty() {
        return "_none_".to_string();
    }
    attempts
        .iter()
        .map(|attempt| {
            format!(
                "{} #{} [{}] exit:{} workdir:{}",
                attempt.attempt_id,
                attempt.attempt,
                attempt.status,
                attempt
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                attempt
                    .workdir
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "none".to_string())
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_attempts(attempts: &[agentflow_core::runtime::AttemptSummary]) -> String {
    if attempts.is_empty() {
        return "_none_".to_string();
    }

    attempts
        .iter()
        .map(|attempt| {
            format!(
                "{} {} {} [{}] {}",
                attempt.attempt_id,
                attempt.run_id,
                attempt.step_id,
                attempt.status,
                attempt.workdir.display()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_environment_check(check: &agentflow_core::runtime::EnvironmentCheckSummary) -> String {
    let status = if check.ok { "ok" } else { "failed" };
    let items = check
        .items
        .iter()
        .map(|item| {
            let details = item
                .details
                .as_deref()
                .map(|details| format!("\n  details: {details}"))
                .unwrap_or_default();
            format!(
                "- {} [{}]: {}{}",
                item.name, item.status, item.message, details
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Environment check\nTool: {}\nVersion: {}\nBackend: {}\nStatus: {}\nItems:\n{}",
        check.tool_ref, check.version, check.backend, status, items
    )
}

fn format_environment_prepare(
    prepare: &agentflow_core::runtime::EnvironmentPrepareSummary,
) -> String {
    let items = prepare
        .items
        .iter()
        .map(|item| {
            let details = item
                .details
                .as_deref()
                .map(|details| format!("\n  details: {details}"))
                .unwrap_or_default();
            format!(
                "- {} [{}]: {}{}",
                item.name, item.status, item.message, details
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let command = if prepare.command.is_empty() {
        "none".to_string()
    } else {
        prepare.command.join(" ")
    };
    format!(
        concat!(
            "Environment prepare\n",
            "Tool: {}\n",
            "Version: {}\n",
            "Backend: {}\n",
            "Status: {}\n",
            "Command: {}\n",
            "Exit code: {}\n",
            "Items:\n{}\n",
            "Stdout:\n{}\n",
            "Stderr:\n{}"
        ),
        prepare.tool_ref,
        prepare.version,
        prepare.backend,
        prepare.status,
        command,
        prepare
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "none".to_string()),
        items,
        prepare.stdout,
        prepare.stderr
    )
}

fn format_environment_export(export: &agentflow_core::runtime::EnvironmentExportSummary) -> String {
    let items = export
        .items
        .iter()
        .map(|item| {
            let details = item
                .details
                .as_deref()
                .map(|details| format!("\n  details: {details}"))
                .unwrap_or_default();
            format!(
                "- {} [{}]: {}{}",
                item.name, item.status, item.message, details
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let command = if export.command.is_empty() {
        "none".to_string()
    } else {
        export.command.join(" ")
    };
    format!(
        concat!(
            "Environment export\n",
            "Tool: {}\n",
            "Version: {}\n",
            "Backend: {}\n",
            "Status: {}\n",
            "Command: {}\n",
            "Exit code: {}\n",
            "Export hash: {}\n",
            "Declared packages: {}\n",
            "Exported packages: {}\n",
            "Missing packages: {}\n",
            "Extra packages: {}\n",
            "Items:\n{}\n",
            "Stdout:\n{}\n",
            "Stderr:\n{}"
        ),
        export.tool_ref,
        export.version,
        export.backend,
        export.status,
        command,
        export
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "none".to_string()),
        export.export_hash.as_deref().unwrap_or("none"),
        format_string_list(&export.declared_packages),
        format_string_list(&export.exported_packages),
        format_string_list(&export.missing_packages),
        format_string_list(&export.extra_packages),
        items,
        export.stdout,
        export.stderr
    )
}

fn format_string_list(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
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

fn format_cache_explanations(
    flow_id: &str,
    explanations: &[agentflow_core::runtime::CacheExplanation],
) -> String {
    if explanations.is_empty() {
        return format!("Cache explain\nFlow: {flow_id}\nNo runnable steps found");
    }

    let details = explanations
        .iter()
        .map(|explanation| {
            let status = if explanation.hit { "hit" } else { "miss" };
            format!(
                "{} [{}]\n  key: {}\n  reason: {}",
                explanation.step_id, status, explanation.cache_key, explanation.reason
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!("Cache explain\nFlow: {flow_id}\n{details}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-cli-{test_name}-{}-{}",
            std::process::id(),
            agentflow_core::storage::now_unix_seconds()
        ))
    }

    fn make_executable(path: &Path) {
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).unwrap();
        }
    }

    fn write_fake_environment_runner(path: &Path) -> PathBuf {
        let runner_path = path.join("fake_micromamba.sh");
        fs::write(
            &runner_path,
            r#"#!/bin/sh
if [ "$1" = "env" ] && [ "$2" = "update" ]; then
  echo "fake env update $*"
  exit 0
fi
if [ "$1" = "env" ] && [ "$2" = "export" ]; then
  printf 'name: af-test\ndependencies:\n  - python=3.11\n  - pandas\n  - scanpy\n'
  exit 0
fi
if [ "$1" != "run" ]; then
  echo "expected run, env update, or env export subcommand" >&2
  exit 91
fi
shift
while [ "$#" -gt 0 ]; do
  case "$1" in
    --name|--prefix)
      shift 2
      ;;
    --no-capture-output)
      shift
      ;;
    *)
      break
      ;;
  esac
done
exec "$@"
"#,
        )
        .unwrap();
        make_executable(&runner_path);
        runner_path
    }

    fn write_synthesizer_stub(path: &std::path::Path, name: &str, output: &str) -> PathBuf {
        let stub_path = path.join(name);
        fs::write(
            &stub_path,
            format!(
                "#!/bin/sh\nprintf '%s' '{}'\n",
                output.replace('\'', "'\\''")
            ),
        )
        .unwrap();
        make_executable(&stub_path);
        stub_path
    }

    fn write_sample_tool(path: &std::path::Path) -> PathBuf {
        let spec_path = path.join("marker_survival_scan.tool.yaml");
        fs::write(
            &spec_path,
            r#"
schema_version: agentflow.tool.v0
namespace: marker
name: marker_survival_scan
version: 0.1.0
maturity: wrapped
description: Scan a candidate marker against survival table
inputs:
  expression_table:
    type: TSV
    required: true
    required_columns: sample,TP53
    sample_id_column: sample
    min_rows: 1
  survival_table:
    type: TSV
    required: true
    required_columns: sample,time,status
    sample_id_column: sample
    min_rows: 1
params:
  gene:
    type: string
    required: true
outputs:
  report:
    type: Markdown
    observer: marker_report
    min_rows: 3
runtime:
  backend: local
  command:
    - /bin/echo
"#,
        )
        .unwrap();
        spec_path
    }

    fn write_executable_sample_tool(path: &std::path::Path) -> PathBuf {
        let script_path = path.join("marker_survival_scan.sh");
        fs::write(
            &script_path,
            r#"cat "$AGENTFLOW_INPUT_EXPRESSION_TABLE" >/dev/null
cat "$AGENTFLOW_INPUT_SURVIVAL_TABLE" >/dev/null
if [ "$AGENTFLOW_PARAM_GENE" = "TP53" ]; then
  score=0.61
else
  score=0.75
fi
printf '# Marker report\nGene: %s\nscore: %s\n' "$AGENTFLOW_PARAM_GENE" "$score" > "$AGENTFLOW_OUTPUT_REPORT"
echo "cli scan ok"
"#,
        )
        .unwrap();
        let spec_path = path.join("marker_survival_scan_executable.tool.yaml");
        fs::write(
            &spec_path,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: marker_survival_scan
version: 0.1.0
maturity: wrapped
description: Scan a candidate marker against survival table
inputs:
  expression_table:
    type: TSV
    required: true
    required_columns: sample,TP53
    sample_id_column: sample
    min_rows: 1
  survival_table:
    type: TSV
    required: true
    required_columns: sample,time,status
    sample_id_column: sample
    min_rows: 1
params:
  gene:
    type: string
    required: true
outputs:
  report:
    type: Markdown
    observer: marker_report
    min_rows: 3
runtime:
  backend: local
  command:
    - /bin/sh
    - {}
"#,
                script_path.display()
            ),
        )
        .unwrap();
        spec_path
    }

    fn write_sample_artifact(path: &std::path::Path, name: &str, contents: &str) -> PathBuf {
        let artifact_path = path.join(name);
        fs::write(&artifact_path, contents).unwrap();
        artifact_path
    }

    fn write_sample_flow(
        path: &std::path::Path,
        expression_artifact_id: &str,
        survival_artifact_id: &str,
    ) -> PathBuf {
        let flow_path = path.join("marker_demo.flow.yaml");
        fs::write(
            &flow_path,
            format!(
                r#"
schema_version: agentflow.flow.v0
id: marker_demo
name: Marker demo
steps:
  - id: scan
    tool: marker/marker_survival_scan
    reason: Evaluate TP53 marker signal
    needs: []
    inputs:
      expression_table: {expression_artifact_id}
      survival_table: {survival_artifact_id}
    params:
      gene: TP53
    outputs:
      report: marker_report
"#
            ),
        )
        .unwrap();
        flow_path
    }

    fn record_cli_hypothesis(path: &std::path::Path, statement: &str) -> String {
        let output = run(args(&[
            "agentflow",
            "hypothesis",
            "create",
            "--statement",
            statement,
            "--origin",
            "test",
            "--goal",
            "goal_test",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        output
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap()
            .to_string()
    }

    fn prepare_approved_marker_flow(path: &std::path::Path) {
        let tool_path = write_sample_tool(path);
        run(args(&[
            "agentflow",
            "tools",
            "register",
            tool_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let expression_path =
            write_sample_artifact(path, "expression.tsv", "sample\tTP53\nA\t1.2\nB\t0.4\n");
        let expression_import = run(args(&[
            "agentflow",
            "import",
            expression_path.to_str().unwrap(),
            "--type",
            "TSV",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let expression_id = expression_import
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();

        let survival_path = write_sample_artifact(
            path,
            "survival.tsv",
            "sample\ttime\tstatus\nA\t10\t1\nB\t12\t0\n",
        );
        let survival_import = run(args(&[
            "agentflow",
            "import",
            survival_path.to_str().unwrap(),
            "--type",
            "TSV",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let survival_id = survival_import
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();

        let flow_path = write_sample_flow(path, expression_id, survival_id);
        run(args(&[
            "agentflow",
            "flow",
            "approve",
            flow_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
    }

    #[test]
    fn version_command_returns_engine_version() {
        let output = run(args(&["agentflow", "--version"])).unwrap();
        assert!(output.starts_with("agentflow "));
    }

    #[test]
    fn no_args_prints_usage() {
        let output = run(args(&["agentflow"])).unwrap();
        assert!(output.contains("Usage:"));
    }

    #[test]
    fn usage_lists_report_cache_and_retry_commands() {
        let output = usage();
        assert!(output.contains("agentflow report <flow-id> [--path <path>]"));
        assert!(output.contains("agentflow report research [--path <path>]"));
        assert!(output.contains("agentflow cache explain <flow-id|step-id> [--path <path>]"));
        assert!(output.contains("agentflow cache list [--json] [--path <path>]"));
        assert!(output.contains(
            "agentflow cache prune (--all|--older-than-seconds <seconds>) [--json] [--path <path>]"
        ));
        assert!(output
            .contains("agentflow run-step <step-id|flow.step|step:flow/step> [--path <path>]"));
        assert!(
            output.contains("agentflow retry <step-id|flow.step|step:flow/step> [--path <path>]")
        );
        assert!(output.contains(
            "agentflow observe <artifact-id> [--adapter artifact_summary|marker_report]"
        ));
        assert!(output.contains("agentflow research list [--json] [--path <path>]"));
        assert!(output.contains("agentflow patch list <flow-id> [--json] [--path <path>]"));
        assert!(output.contains("agentflow patch apply <patch-id> [--json] [--path <path>]"));
        assert!(output.contains("agentflow compare steps <flow-id>"));
        assert!(output.contains("agentflow compare metrics <flow-id>"));
        assert!(output.contains("agentflow runs list [--flow <flow-id>]"));
        assert!(output.contains("agentflow runs inspect <run-or-attempt-id>"));
        assert!(output.contains("agentflow env check <tool-ref>"));
        assert!(output.contains("agentflow env prepare <tool-ref>"));
        assert!(output.contains("agentflow env export <tool-ref>"));
        assert!(output.contains("agentflow llm config --provider anthropic|openai|gemini|deepseek"));
    }

    #[test]
    fn unknown_command_is_error() {
        let err = run(args(&["agentflow", "unknown"])).unwrap_err();
        assert!(err.message().contains("unrecognized subcommand 'unknown'"));
    }

    #[test]
    fn init_and_status_work_with_explicit_path() {
        let path = temp_project_path("init-status");
        let init = run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(init.contains("Initialized AgentFlow project"));

        let status = run(args(&[
            "agentflow",
            "status",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(status.contains("\"schema_version\":\"agentflow.status.v0\""));
        assert!(status.contains("\"name\":\"Demo\""));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn llm_config_writes_secret_env_without_leaking_output() {
        let path = temp_project_path("llm-config");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let output = run(args(&[
            "agentflow",
            "llm",
            "config",
            "--provider",
            "anthropic",
            "--api-key",
            "test-secret-1234",
            "--model",
            "claude-test-model",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(output.contains("\"schema_version\":\"agentflow.llm_config.v0\""));
        assert!(output.contains("\"api_key_var\":\"ANTHROPIC_API_KEY\""));
        assert!(!output.contains("test-secret-1234"));
        let env_path = path.join(".agentflow/llm.env");
        let env = fs::read_to_string(&env_path).unwrap();
        assert!(env.contains("AGENTFLOW_LLM_PROVIDER='anthropic'"));
        assert!(env.contains("ANTHROPIC_API_KEY='test-secret-1234'"));
        assert!(env.contains("ANTHROPIC_MODEL='claude-test-model'"));
        assert!(path.join(".agentflow/llm-synth.sh").exists());
        assert!(path.join(".agentflow/llm-synth.py").exists());
        let synth_py = fs::read_to_string(path.join(".agentflow/llm-synth.py")).unwrap();
        assert!(synth_py.contains("LLM connection error:"));
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&env_path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn llm_config_deepseek_defaults_model_and_writes_base_url() {
        let path = temp_project_path("llm-config-deepseek");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let output = run(args(&[
            "agentflow",
            "llm",
            "config",
            "--provider",
            "deepseek",
            "--api-key",
            "test-deepseek-secret",
            "--base-url",
            "https://api.deepseek.com/v1",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(output.contains("\"provider\":\"deepseek\""));
        assert!(output.contains("\"api_key_var\":\"DEEPSEEK_API_KEY\""));
        assert!(output.contains("\"model\":\"deepseek-v4-flash\""));
        assert!(output.contains("\"base_url_configured\":true"));
        assert!(!output.contains("test-deepseek-secret"));
        let env = fs::read_to_string(path.join(".agentflow/llm.env")).unwrap();
        assert!(env.contains("AGENTFLOW_LLM_PROVIDER='deepseek'"));
        assert!(env.contains("DEEPSEEK_API_KEY='test-deepseek-secret'"));
        assert!(env.contains("DEEPSEEK_MODEL='deepseek-v4-flash'"));
        assert!(env.contains("DEEPSEEK_BASE_URL='https://api.deepseek.com/v1'"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn llm_config_deepseek_accepts_explicit_v4_pro_model() {
        let path = temp_project_path("llm-config-deepseek-pro");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let output = run(args(&[
            "agentflow",
            "llm",
            "config",
            "--provider",
            "deepseek",
            "--api-key",
            "test-deepseek-secret",
            "--model",
            "deepseek-v4-pro",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(output.contains("\"model\":\"deepseek-v4-pro\""));
        let env = fs::read_to_string(path.join(".agentflow/llm.env")).unwrap();
        assert!(env.contains("DEEPSEEK_MODEL='deepseek-v4-pro'"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn llm_config_generated_shell_exports_env_to_python() {
        let path = temp_project_path("llm-config-shell-export");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        run(args(&[
            "agentflow",
            "llm",
            "config",
            "--provider",
            "deepseek",
            "--api-key",
            "test-deepseek-secret",
            "--base-url",
            "https://api.deepseek.com/v1",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        fs::write(
            path.join(".agentflow/llm-synth.py"),
            r#"#!/usr/bin/env python3
import os
print(os.environ["AGENTFLOW_LLM_PROVIDER"])
print(os.environ["DEEPSEEK_MODEL"])
print(os.environ["DEEPSEEK_BASE_URL"])
"#,
        )
        .unwrap();

        let output = std::process::Command::new(path.join(".agentflow/llm-synth.sh"))
            .arg("unused")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("deepseek\n"));
        assert!(stdout.contains("deepseek-v4-flash\n"));
        assert!(stdout.contains("https://api.deepseek.com/v1\n"));
        assert!(!stdout.contains("test-deepseek-secret"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn llm_config_default_synthesizer_survives_paths_with_spaces() {
        let path = temp_project_path("llm config with spaces");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let output = run(args(&[
            "agentflow",
            "llm",
            "config",
            "--provider",
            "anthropic",
            "--api-key",
            "test-secret-1234",
            "--model",
            "claude-test-model",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(!output.contains("test-secret-1234"));
        let configured =
            crate::synth_commands::configured_or_default_synthesizer(&path, None).unwrap();
        let argv = crate::synth_commands::split_synthesizer_command(&configured).unwrap();
        let expected_root = fs::canonicalize(&path).unwrap();
        assert_eq!(
            argv,
            vec![expected_root
                .join(".agentflow/llm-synth.sh")
                .display()
                .to_string()]
        );

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn configured_llm_env_drives_default_synthesizer() {
        let path = temp_project_path("llm-env-synth");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let fixture = path.join("fixture.txt");
        fs::write(&fixture, "expected-line\n").unwrap();
        let stub = path.join("env_synth.sh");
        fs::write(
            &stub,
            r#"#!/bin/sh
if [ "${ANTHROPIC_API_KEY:-}" != "test-secret-1234" ]; then
  echo "missing configured key" >&2
  exit 7
fi
cat <<'PY'
import os
from pathlib import Path
print(Path(os.environ["SYNTH_INPUT"]).read_text(), end="")
PY
"#,
        )
        .unwrap();
        make_executable(&stub);
        let synthesizer = format!("/bin/sh {}", stub.display());
        run(args(&[
            "agentflow",
            "llm",
            "config",
            "--provider",
            "anthropic",
            "--api-key",
            "test-secret-1234",
            "--model",
            "claude-test-model",
            "--synthesizer",
            &synthesizer,
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let output = run(args(&[
            "agentflow",
            "synth",
            "--name",
            "llm_env_echo",
            "--description",
            "Echo the fixture",
            "--fixture",
            fixture.to_str().unwrap(),
            "--expect",
            "expected-line",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(output.contains("VALIDATED"));
        assert!(output.contains("synth/llm_env_echo"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn tools_register_list_and_inspect_work_with_explicit_path() {
        let path = temp_project_path("tools-register-list-inspect");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let spec_path = write_sample_tool(&path);

        let register = run(args(&[
            "agentflow",
            "tools",
            "register",
            spec_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(register.contains("Registered tool"));
        assert!(register.contains("marker/marker_survival_scan"));

        let list = run(args(&[
            "agentflow",
            "tools",
            "list",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(list.contains("\"schema_version\":\"agentflow.tool_list.v0\""));
        assert!(list.contains("\"ref\":\"marker/marker_survival_scan\""));

        let inspect = run(args(&[
            "agentflow",
            "tools",
            "inspect",
            "marker/marker_survival_scan",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(inspect.contains("\"schema_version\":\"agentflow.tool_inspection.v0\""));
        assert!(inspect.contains("\"version\":\"0.1.0\""));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn tools_match_and_draft_step_json_work_with_registered_tool() {
        let path = temp_project_path("tools-match-draft-json");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let spec_path = write_sample_tool(&path);
        run(args(&[
            "agentflow",
            "tools",
            "register",
            spec_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let matches = run(args(&[
            "agentflow",
            "tools",
            "match",
            "--output",
            "Markdown",
            "--input",
            "TSV",
            "--keyword",
            "survival",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(matches.starts_with('['));
        assert!(matches.contains("\"tool_ref\":\"marker/marker_survival_scan\""));
        assert!(matches.contains("\"fit\":\"high\""));
        assert!(matches.contains("\"score\":23"));

        let draft = run(args(&[
            "agentflow",
            "tools",
            "draft-step",
            "marker/marker_survival_scan",
            "--input",
            "TSV:artifact_table",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(draft.contains("\"id\":\"step_marker_survival_scan\""));
        assert!(draft.contains("\"tool\":\"marker/marker_survival_scan\""));
        assert!(draft.contains("\"expression_table\":\"artifact_table\""));
        assert!(draft.contains("\"survival_table\":\"artifact_table\""));
        assert!(draft.contains("\"gene\":\"REPLACE_gene\""));
        assert!(draft.contains("\"report\":\"step_marker_survival_scan_report\""));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn tools_match_and_draft_step_human_output_work() {
        let path = temp_project_path("tools-match-draft-human");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let spec_path = write_sample_tool(&path);
        run(args(&[
            "agentflow",
            "tools",
            "register",
            spec_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let matches = run(args(&[
            "agentflow",
            "tools",
            "match",
            "--output",
            "Markdown",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(matches.contains("marker/marker_survival_scan [medium] score=11"));
        assert!(matches.contains("reason=output:Markdown, maturity:wrapped"));

        let draft = run(args(&[
            "agentflow",
            "tools",
            "draft-step",
            "marker/marker_survival_scan",
            "--input",
            "TSV:artifact_table",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(draft.contains("Step: step_marker_survival_scan"));
        assert!(draft.contains("Tool: marker/marker_survival_scan"));
        assert!(draft.contains("expression_table: artifact_table"));
        assert!(draft.contains("gene: REPLACE_gene"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn tools_draft_step_propagates_missing_tool_error() {
        let path = temp_project_path("tools-draft-missing");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let error = run(args(&[
            "agentflow",
            "tools",
            "draft-step",
            "missing/tool",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();

        assert!(error.message().contains("not found: tool missing/tool"));
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn tools_draft_step_infers_replace_param_from_hypothesis_stub() {
        let path = temp_project_path("tools-draft-infer-param");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let spec_path = write_sample_tool(&path);
        run(args(&[
            "agentflow",
            "tools",
            "register",
            spec_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let hypothesis_id =
            record_cli_hypothesis(&path, "THRSP is a candidate survival-associated marker");
        let stub_path = write_synthesizer_stub(&path, "synth-thrsp.sh", "THRSP\n");

        let draft = run(args(&[
            "agentflow",
            "tools",
            "draft-step",
            "marker/marker_survival_scan",
            "--input",
            "TSV:artifact_table",
            "--hypothesis",
            &hypothesis_id,
            "--infer-params",
            "--synthesizer",
            stub_path.to_str().unwrap(),
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(draft.contains("\"gene\":\"THRSP\""));
        assert!(!draft.contains("\"gene\":\"REPLACE_gene\""));
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn tools_draft_step_keeps_placeholder_when_inference_is_empty() {
        let path = temp_project_path("tools-draft-infer-empty");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let spec_path = write_sample_tool(&path);
        run(args(&[
            "agentflow",
            "tools",
            "register",
            spec_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let hypothesis_id =
            record_cli_hypothesis(&path, "No unambiguous marker should be inferred");
        let stub_path = write_synthesizer_stub(&path, "synth-empty.sh", "\n");

        let draft = run(args(&[
            "agentflow",
            "tools",
            "draft-step",
            "marker/marker_survival_scan",
            "--input",
            "TSV:artifact_table",
            "--hypothesis",
            &hypothesis_id,
            "--infer-params",
            "--synthesizer",
            stub_path.to_str().unwrap(),
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(draft.contains("\"gene\":\"REPLACE_gene\""));
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn tools_draft_step_infer_params_requires_hypothesis() {
        let error = run(args(&[
            "agentflow",
            "tools",
            "draft-step",
            "marker/marker_survival_scan",
            "--infer-params",
        ]))
        .unwrap_err();

        assert!(matches!(error, CliError::InvalidArgument(_)));
        assert!(error
            .message()
            .contains("--infer-params requires --hypothesis <id>"));
    }

    #[test]
    fn tools_register_resolves_relative_runtime_script_args() {
        let path = temp_project_path("tools-register-relative-script");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let tool_dir = path.join("tools");
        fs::create_dir_all(&tool_dir).unwrap();
        let script_path = tool_dir.join("relative_marker.sh");
        fs::write(&script_path, "echo ok\n").unwrap();
        let spec_path = tool_dir.join("relative_marker.tool.yaml");
        fs::write(
            &spec_path,
            r#"
schema_version: agentflow.tool.v0
namespace: marker
name: relative_marker
version: 0.1.0
maturity: wrapped
description: Tool with a script path relative to the tool spec
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/sh
    - relative_marker.sh
"#,
        )
        .unwrap();

        run(args(&[
            "agentflow",
            "tools",
            "register",
            spec_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let inspect = run(args(&[
            "agentflow",
            "tools",
            "inspect",
            "marker/relative_marker",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(inspect.contains(&script_path.canonicalize().unwrap().display().to_string()));

        let env_file = tool_dir.join("environment.yml");
        fs::write(&env_file, "name: relative-env\n").unwrap();
        let env_spec_path = tool_dir.join("relative_env.tool.yaml");
        fs::write(
            &env_spec_path,
            r#"
schema_version: agentflow.tool.v0
namespace: marker
name: relative_env
version: 0.1.0
maturity: wrapped
description: Tool with an env file relative to the tool spec
outputs:
  report:
    type: Markdown
runtime:
  backend: conda
  runner: /bin/echo
  env_name: relative-env
  env_file: environment.yml
  command:
    - python
    - run.py
"#,
        )
        .unwrap();
        run(args(&[
            "agentflow",
            "tools",
            "register",
            env_spec_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let env_inspect = run(args(&[
            "agentflow",
            "tools",
            "inspect",
            "marker/relative_env",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(env_inspect.contains(&env_file.canonicalize().unwrap().display().to_string()));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn env_check_reports_existing_environment_wrapper() {
        let path = temp_project_path("env-check");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let spec_path = path.join("env_tool.tool.yaml");
        fs::write(
            &spec_path,
            r#"
schema_version: agentflow.tool.v0
namespace: marker
name: env_tool
version: 0.1.0
maturity: wrapped
description: Tool with an existing environment wrapper
outputs:
  report:
    type: Markdown
runtime:
  backend: micromamba
  runner: /bin/echo
  env_name: af-test
  command:
    - python
    - run.py
"#,
        )
        .unwrap();
        run(args(&[
            "agentflow",
            "tools",
            "register",
            spec_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let output = run(args(&[
            "agentflow",
            "env",
            "check",
            "marker/env_tool",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(output.contains("\"schema_version\":\"agentflow.env_check.v0\""));
        assert!(output.contains("\"backend\":\"micromamba\""));
        assert!(output.contains("\"ok\":true"));
        assert!(output.contains("\"name\":\"probe\""));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn env_prepare_reports_explicit_environment_update() {
        let path = temp_project_path("env-prepare");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let env_file = path.join("environment.yml");
        fs::write(&env_file, "name: af-test\n").unwrap();
        let spec_path = path.join("env_tool.tool.yaml");
        fs::write(
            &spec_path,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: env_tool
version: 0.1.0
maturity: wrapped
description: Tool with an existing environment wrapper
outputs:
  report:
    type: Markdown
runtime:
  backend: conda
  runner: /bin/echo
  env_name: af-test
  env_file: {}
  command:
    - python
    - run.py
"#,
                env_file.display()
            ),
        )
        .unwrap();
        run(args(&[
            "agentflow",
            "tools",
            "register",
            spec_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let output = run(args(&[
            "agentflow",
            "env",
            "prepare",
            "marker/env_tool",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(output.contains("\"schema_version\":\"agentflow.env_prepare.v0\""));
        assert!(output.contains("\"backend\":\"conda\""));
        assert!(output.contains("\"status\":\"succeeded\""));
        assert!(output.contains("\"ok\":true"));
        assert!(output.contains("\"command\":[\"/bin/echo\",\"env\",\"update\""));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn env_export_reports_environment_lock_and_package_diff() {
        let path = temp_project_path("env-export");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let runner_path = write_fake_environment_runner(&path);
        let env_file = path.join("environment.yml");
        fs::write(
            &env_file,
            "name: af-test\ndependencies:\n  - python=3.11\n  - pandas\n  - numpy\n",
        )
        .unwrap();
        let spec_path = path.join("env_tool.tool.yaml");
        fs::write(
            &spec_path,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: env_tool
version: 0.1.0
maturity: wrapped
description: Tool with an exportable environment wrapper
outputs:
  report:
    type: Markdown
runtime:
  backend: micromamba
  runner: {}
  env_name: af-test
  env_file: {}
  command:
    - python
    - run.py
"#,
                runner_path.display(),
                env_file.display()
            ),
        )
        .unwrap();
        run(args(&[
            "agentflow",
            "tools",
            "register",
            spec_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let output = run(args(&[
            "agentflow",
            "env",
            "export",
            "marker/env_tool",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        assert!(output.contains("\"schema_version\":\"agentflow.env_export.v0\""));
        assert!(output.contains("\"backend\":\"micromamba\""));
        assert!(output.contains("\"status\":\"succeeded\""));
        assert!(output.contains("\"ok\":false"));
        assert!(output.contains("\"export_hash\":\"fnv64:"));
        assert!(output.contains("\"missing_packages\":[\"numpy\"]"));
        assert!(output.contains("\"extra_packages\":[\"scanpy\"]"));
        assert!(output.contains("\"command\":[\""));
        assert!(output.contains("\",\"env\",\"export\""));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn import_list_and_inspect_artifact_work_with_explicit_path() {
        let path = temp_project_path("import-list-inspect");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let artifact_path =
            write_sample_artifact(&path, "expression.tsv", "sample\tTP53\nA\t1.2\nB\t0.4\n");

        let import = run(args(&[
            "agentflow",
            "import",
            artifact_path.to_str().unwrap(),
            "--type",
            "TSV",
            "--mode",
            "reference",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(import.contains("Imported artifact"));
        assert!(import.contains("Type: TSV"));

        let list = run(args(&[
            "agentflow",
            "artifacts",
            "list",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(list.contains("\"schema_version\":\"agentflow.artifact_list.v0\""));
        assert!(list.contains("\"type\":\"TSV\""));

        let artifact_id = list
            .split("\"id\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap();
        let inspect = run(args(&[
            "agentflow",
            "artifacts",
            "inspect",
            artifact_id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(inspect.contains("\"schema_version\":\"agentflow.artifact_inspection.v0\""));
        assert!(inspect.contains("\"import_mode\":\"reference\""));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn observe_list_and_inspect_work_with_explicit_path() {
        let path = temp_project_path("observe-list-inspect");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let artifact_path =
            write_sample_artifact(&path, "expression.tsv", "sample\tTP53\nA\t1.2\nB\t0.4\n");
        let import = run(args(&[
            "agentflow",
            "import",
            artifact_path.to_str().unwrap(),
            "--type",
            "TSV",
            "--mode",
            "reference",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let artifact_id = import
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();

        let observation = run(args(&[
            "agentflow",
            "observe",
            artifact_id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(observation.contains("\"kind\":\"artifact_summary\""));
        assert!(observation.contains("\"line_count\":3"));

        let observation_id = format!("observation_artifact_summary_{artifact_id}");
        let list = run(args(&[
            "agentflow",
            "observations",
            "list",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(list.contains("\"schema_version\":\"agentflow.observation_list.v0\""));
        assert!(list.contains(&observation_id));

        let inspect = run(args(&[
            "agentflow",
            "observations",
            "inspect",
            &observation_id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(inspect.contains("\"artifact_id\""));
        assert!(inspect.contains("sample\\tTP53"));

        let marker_path = write_sample_artifact(
            &path,
            "marker.md",
            "# Candidate marker\nGene: EGFR\nscore: 0.75\n",
        );
        let marker_import = run(args(&[
            "agentflow",
            "import",
            marker_path.to_str().unwrap(),
            "--type",
            "Markdown",
            "--mode",
            "reference",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let marker_artifact_id = marker_import
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();
        let marker_observation = run(args(&[
            "agentflow",
            "observe",
            marker_artifact_id,
            "--adapter",
            "marker_report",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(marker_observation.contains("\"kind\":\"marker_report\""));
        assert!(marker_observation.contains("\"adapter\":\"marker_report\""));
        assert!(marker_observation.contains("\"domain\":{\"gene\":\"EGFR\""));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn research_note_list_and_inspect_work_with_explicit_path() {
        let path = temp_project_path("research-note-list-inspect");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let note = run(args(&[
            "agentflow",
            "research",
            "note",
            "--problem",
            "Candidate marker failed validation",
            "--question",
            "Should homolog genes be considered?",
            "--finding",
            "A homolog remains plausible in the pathway.",
            "--confidence",
            "medium",
            "--source",
            "local notes",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(note.contains("Recorded research note"));
        let note_id = note
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();

        let list = run(args(&[
            "agentflow",
            "research",
            "list",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(list.contains("\"schema_version\":\"agentflow.research_notes.v0\""));
        assert!(list.contains("Candidate marker failed validation"));
        assert!(list.contains(note_id));

        let inspect = run(args(&[
            "agentflow",
            "research",
            "inspect",
            note_id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(inspect.contains("\"confidence\":\"medium\""));
        assert!(inspect.contains("A homolog remains plausible"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn flow_validate_approve_and_inspect_work_with_explicit_path() {
        let path = temp_project_path("flow-validate-approve-inspect");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let tool_path = write_sample_tool(&path);
        run(args(&[
            "agentflow",
            "tools",
            "register",
            tool_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let artifact_path =
            write_sample_artifact(&path, "expression.tsv", "sample\tTP53\nA\t1.2\nB\t0.4\n");
        let import = run(args(&[
            "agentflow",
            "import",
            artifact_path.to_str().unwrap(),
            "--type",
            "TSV",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let artifact_id = import
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();
        let survival_path =
            write_sample_artifact(&path, "survival.tsv", "sample\ttime\tstatus\nA\t10\t1\n");
        let survival_import = run(args(&[
            "agentflow",
            "import",
            survival_path.to_str().unwrap(),
            "--type",
            "TSV",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let survival_artifact_id = survival_import
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();
        let flow_path = write_sample_flow(&path, artifact_id, survival_artifact_id);

        let validate = run(args(&[
            "agentflow",
            "flow",
            "validate",
            flow_path.to_str().unwrap(),
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(validate.contains("\"schema_version\":\"agentflow.flow_validation.v0\""));
        assert!(validate.contains("\"valid\":true"));

        let approve = run(args(&[
            "agentflow",
            "flow",
            "approve",
            flow_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(approve.contains("Approved flow"));
        assert!(approve.contains("marker_demo"));

        let inspect = run(args(&[
            "agentflow",
            "flow",
            "inspect",
            "marker_demo",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(inspect.contains("\"schema_version\":\"agentflow.flow_inspection.v0\""));
        assert!(inspect.contains("\"status\":\"approved\""));
        assert!(inspect.contains("\"local_id\":\"scan\""));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn patch_propose_list_approve_and_reject_work_with_explicit_path() {
        let path = temp_project_path("patch-propose-list-approve-reject");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        prepare_approved_marker_flow(&path);
        let artifacts = run(args(&[
            "agentflow",
            "artifacts",
            "list",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let artifact_ids = artifacts
            .split("\"id\":\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .collect::<Vec<_>>();
        let expression_id = artifact_ids[0];
        let survival_id = artifact_ids[1];
        let patch_json = format!(
            r#"{{"ops":[{{"op":"add_step","id":"ortholog_scan","tool":"marker/marker_survival_scan","reason":"Evaluate related marker","needs":["scan"],"inputs":{{"expression_table":"{expression_id}","survival_table":"{survival_id}"}},"params":{{"gene":"EGFR"}},"outputs":{{"report":"ortholog_report"}}}}]}}"#
        );

        let proposed = run(args(&[
            "agentflow",
            "patch",
            "propose",
            "marker_demo",
            "--title",
            "Add ortholog branch",
            "--reason",
            "Primary marker was weak; compare a related hypothesis.",
            "--patch-json",
            patch_json.as_str(),
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(proposed.contains("\"status\":\"pending\""));
        assert!(proposed.contains("Add ortholog branch"));
        let patch_id = proposed
            .split("\"id\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap();

        let list = run(args(&[
            "agentflow",
            "patch",
            "list",
            "marker_demo",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(list.contains("\"schema_version\":\"agentflow.graph_patch_list.v0\""));
        assert!(list.contains(patch_id));
        assert!(list.contains("\"status\":\"pending\""));

        let approved = run(args(&[
            "agentflow",
            "patch",
            "approve",
            patch_id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(approved.contains("\"status\":\"approved\""));

        let applied = run(args(&[
            "agentflow",
            "patch",
            "apply",
            patch_id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(applied.contains("\"schema_version\":\"agentflow.graph_patch_application.v0\""));
        assert!(applied.contains("\"ortholog_scan\""));
        assert!(applied.contains("\"updated_steps\":[]"));
        assert!(applied.contains("\"invalidated_steps\":[]"));

        let inspect = run(args(&[
            "agentflow",
            "flow",
            "inspect",
            "marker_demo",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(inspect.contains("\"local_id\":\"ortholog_scan\""));

        let update = run(args(&[
            "agentflow",
            "patch",
            "propose",
            "marker_demo",
            "--title",
            "Retest marker",
            "--reason",
            "Replay the branch with a different marker parameter.",
            "--patch-json",
            r#"{"ops":[{"op":"update_params","step":"scan","params":{"gene":"ALK"}}]}"#,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let update_patch_id = update
            .split("\"id\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap();
        run(args(&[
            "agentflow",
            "patch",
            "approve",
            update_patch_id,
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let update_applied = run(args(&[
            "agentflow",
            "patch",
            "apply",
            update_patch_id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(update_applied.contains("\"updated_steps\":[\"scan\"]"));
        assert!(update_applied.contains("\"invalidated_steps\":[\"scan\",\"ortholog_scan\"]"));

        let updated_inspect = run(args(&[
            "agentflow",
            "flow",
            "inspect",
            "marker_demo",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(updated_inspect.contains("\"params\":{\"gene\":\"ALK\"}"));

        let second = run(args(&[
            "agentflow",
            "patch",
            "propose",
            "marker_demo",
            "--title",
            "Reject unsafe branch",
            "--reason",
            "It skips the explicit review step.",
            "--patch-json",
            r#"{"ops":[{"op":"remove_step","id":"review"}]}"#,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let rejected_patch_id = second
            .split("\"id\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap();
        let rejected = run(args(&[
            "agentflow",
            "patch",
            "reject",
            rejected_patch_id,
            "--reason",
            "Review gate must stay explicit.",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(rejected.contains("\"status\":\"rejected\""));
        assert!(rejected.contains("Review gate must stay explicit"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn applied_patch_can_run_new_branch_and_record_comparison() {
        let path = temp_project_path("patch-run-compare");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let tool_path = write_executable_sample_tool(&path);
        run(args(&[
            "agentflow",
            "tools",
            "register",
            tool_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let expression_path =
            write_sample_artifact(&path, "expression.tsv", "sample\tTP53\tEGFR\nA\t1.2\t0.8\n");
        let expression_import = run(args(&[
            "agentflow",
            "import",
            expression_path.to_str().unwrap(),
            "--type",
            "TSV",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let expression_id = expression_import
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();
        let survival_path =
            write_sample_artifact(&path, "survival.tsv", "sample\ttime\tstatus\nA\t10\t1\n");
        let survival_import = run(args(&[
            "agentflow",
            "import",
            survival_path.to_str().unwrap(),
            "--type",
            "TSV",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let survival_id = survival_import
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();
        let flow_path = write_sample_flow(&path, expression_id, survival_id);
        run(args(&[
            "agentflow",
            "flow",
            "approve",
            flow_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let first_run = run(args(&[
            "agentflow",
            "run",
            "marker_demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(first_run.contains("Completed steps: 1"));

        let patch_json = format!(
            r#"{{"ops":[{{"op":"add_step","id":"ortholog_scan","tool":"marker/marker_survival_scan","reason":"Evaluate related marker after baseline","needs":["scan"],"inputs":{{"expression_table":"{expression_id}","survival_table":"{survival_id}"}},"params":{{"gene":"EGFR"}},"outputs":{{"report":"ortholog_report"}}}}]}}"#
        );
        let proposed = run(args(&[
            "agentflow",
            "patch",
            "propose",
            "marker_demo",
            "--title",
            "Add ortholog branch",
            "--reason",
            "Baseline completed; compare related marker.",
            "--patch-json",
            patch_json.as_str(),
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let patch_id = proposed
            .split("\"id\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap();
        run(args(&[
            "agentflow",
            "patch",
            "approve",
            patch_id,
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        run(args(&[
            "agentflow",
            "patch",
            "apply",
            patch_id,
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let second_run = run(args(&[
            "agentflow",
            "run",
            "marker_demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(second_run.contains("Completed steps: 1"));
        assert!(second_run.contains("ortholog_scan"));

        let metric_comparison = run(args(&[
            "agentflow",
            "compare",
            "metrics",
            "marker_demo",
            "--baseline",
            "scan",
            "--candidate",
            "ortholog_scan",
            "--metric",
            "score",
            "--direction",
            "higher",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(metric_comparison.contains("\"winner\":\"candidate\""));
        assert!(metric_comparison.contains("\"baseline_value\":0.61"));
        assert!(metric_comparison.contains("\"candidate_value\":0.75"));

        let comparison = run(args(&[
            "agentflow",
            "compare",
            "steps",
            "marker_demo",
            "--baseline",
            "scan",
            "--candidate",
            "ortholog_scan",
            "--summary",
            "Ortholog branch ran successfully but needs biological validation.",
            "--winner",
            "inconclusive",
            "--reason",
            "Runtime completion alone is insufficient evidence.",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(comparison.contains("\"winner\":\"inconclusive\""));
        let comparison_id = comparison
            .split("\"id\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap();

        let list = run(args(&[
            "agentflow",
            "compare",
            "list",
            "marker_demo",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(list.contains("\"schema_version\":\"agentflow.branch_comparison_list.v0\""));
        assert!(list.contains(comparison_id));

        let update_patch = run(args(&[
            "agentflow",
            "patch",
            "propose",
            "marker_demo",
            "--title",
            "Retest baseline marker",
            "--reason",
            "Replay the completed branch with a revised marker parameter.",
            "--patch-json",
            r#"{"ops":[{"op":"update_params","step":"scan","params":{"gene":"ALK"}}]}"#,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let update_patch_id = update_patch
            .split("\"id\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap();
        run(args(&[
            "agentflow",
            "patch",
            "approve",
            update_patch_id,
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let update_application = run(args(&[
            "agentflow",
            "patch",
            "apply",
            update_patch_id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(update_application.contains("\"updated_steps\":[\"scan\"]"));
        assert!(update_application.contains("\"invalidated_steps\":[\"scan\",\"ortholog_scan\"]"));

        let replay = run(args(&[
            "agentflow",
            "run",
            "marker_demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(replay.contains("Completed steps: 2"));
        assert!(replay.contains("scan"));
        assert!(replay.contains("ortholog_scan"));

        let inspect = run(args(&[
            "agentflow",
            "compare",
            "inspect",
            comparison_id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(inspect.contains("Ortholog branch ran successfully"));

        let report = run(args(&[
            "agentflow",
            "report",
            "marker_demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(report.contains("### Branch Comparisons"));
        assert!(report.contains("ortholog_scan"));
        assert!(report.contains("Metric `score`"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn run_step_work_with_explicit_path() {
        let path = temp_project_path("run-step");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let tool_path = write_executable_sample_tool(&path);
        run(args(&[
            "agentflow",
            "tools",
            "register",
            tool_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let expression_path =
            write_sample_artifact(&path, "expression.tsv", "sample\tTP53\nA\t1.2\nB\t0.4\n");
        let expression_import = run(args(&[
            "agentflow",
            "import",
            expression_path.to_str().unwrap(),
            "--type",
            "TSV",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let expression_id = expression_import
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();
        let survival_path = write_sample_artifact(
            &path,
            "survival.tsv",
            "sample\ttime\tstatus\nA\t10\t1\nB\t12\t0\n",
        );
        let survival_import = run(args(&[
            "agentflow",
            "import",
            survival_path.to_str().unwrap(),
            "--type",
            "TSV",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let survival_id = survival_import
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();
        let flow_path = write_sample_flow(&path, expression_id, survival_id);
        run(args(&[
            "agentflow",
            "flow",
            "approve",
            flow_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let output = run(args(&[
            "agentflow",
            "run-step",
            "marker_demo.scan",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(output.contains("Run step complete"));
        assert!(output.contains("Completed steps: 1"));
        assert!(output.contains("Failed steps: 0"));

        let inspect = run(args(&[
            "agentflow",
            "flow",
            "inspect",
            "marker_demo",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(inspect.contains("\"local_id\":\"scan\""));
        assert!(inspect.contains("\"status\":\"completed\""));

        let observations = run(args(&[
            "agentflow",
            "observations",
            "list",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(observations.contains("\"kind\":\"marker_report\""));

        let rerun = run(args(&[
            "agentflow",
            "run-step",
            "marker_demo.scan",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(rerun.message().contains("run-step supports draft"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn run_and_logs_work_with_explicit_path() {
        let path = temp_project_path("run-logs");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let tool_path = write_executable_sample_tool(&path);
        run(args(&[
            "agentflow",
            "tools",
            "register",
            tool_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let expression_path =
            write_sample_artifact(&path, "expression.tsv", "sample\tTP53\nA\t1.2\nB\t0.4\n");
        let expression_import = run(args(&[
            "agentflow",
            "import",
            expression_path.to_str().unwrap(),
            "--type",
            "TSV",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let expression_id = expression_import
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();
        let survival_path = write_sample_artifact(
            &path,
            "survival.tsv",
            "sample\ttime\tstatus\nA\t10\t1\nB\t12\t0\n",
        );
        let survival_import = run(args(&[
            "agentflow",
            "import",
            survival_path.to_str().unwrap(),
            "--type",
            "TSV",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let survival_id = survival_import
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();
        let flow_path = write_sample_flow(&path, expression_id, survival_id);
        run(args(&[
            "agentflow",
            "flow",
            "approve",
            flow_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let run_output = run(args(&[
            "agentflow",
            "run",
            "marker_demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(run_output.contains("Completed steps: 1"));
        assert!(run_output.contains("Failed steps: 0"));
        assert!(run_output.contains(" [succeeded] "));
        let attempt_id = run_output
            .lines()
            .find(|line| line.starts_with("attempt_"))
            .and_then(|line| line.split_whitespace().next())
            .unwrap();
        let run_id = run_output
            .lines()
            .find(|line| line.starts_with("attempt_"))
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap();

        let runs = run(args(&[
            "agentflow",
            "runs",
            "list",
            "--flow",
            "marker_demo",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(runs.contains("\"schema_version\":\"agentflow.runs.v0\""));
        assert!(runs.contains("\"status\":\"completed\""));
        assert!(runs.contains("\"attempt_count\":1"));

        let run_inspect = run(args(&[
            "agentflow",
            "runs",
            "inspect",
            run_id,
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(run_inspect.contains("\"schema_version\":\"agentflow.run_inspection.v0\""));
        assert!(run_inspect.contains("\"status\":\"succeeded\""));

        let attempt_inspect = run(args(&[
            "agentflow",
            "runs",
            "inspect",
            attempt_id,
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(attempt_inspect.contains("Run: run_"));
        assert!(attempt_inspect.contains("[succeeded]"));

        let logs = run(args(&[
            "agentflow",
            "logs",
            attempt_id,
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(logs.contains("cli scan ok"));

        let artifacts = run(args(&[
            "agentflow",
            "artifacts",
            "list",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(artifacts.contains("\"kind\":\"computed\""));

        let status = run(args(&[
            "agentflow",
            "status",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(status.contains("\"run_attempts\":1"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn cache_explain_reports_hits_for_flow_ids() {
        let path = temp_project_path("cache-explain");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let tool_path = write_executable_sample_tool(&path);
        run(args(&[
            "agentflow",
            "tools",
            "register",
            tool_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let expression_path =
            write_sample_artifact(&path, "expression.tsv", "sample\tTP53\nA\t1.2\nB\t0.4\n");
        let expression_import = run(args(&[
            "agentflow",
            "import",
            expression_path.to_str().unwrap(),
            "--type",
            "TSV",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let expression_id = expression_import
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();
        let survival_path = write_sample_artifact(
            &path,
            "survival.tsv",
            "sample\ttime\tstatus\nA\t10\t1\nB\t12\t0\n",
        );
        let survival_import = run(args(&[
            "agentflow",
            "import",
            survival_path.to_str().unwrap(),
            "--type",
            "TSV",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let survival_id = survival_import
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();
        let flow_path = write_sample_flow(&path, expression_id, survival_id);
        run(args(&[
            "agentflow",
            "flow",
            "approve",
            flow_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        run(args(&[
            "agentflow",
            "run",
            "marker_demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let explain = run(args(&[
            "agentflow",
            "cache",
            "explain",
            "marker_demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(explain.contains("Cache explain"));
        assert!(explain.contains("Flow: marker_demo"));
        assert!(explain.contains("scan [hit]"));

        let step_explain = run(args(&[
            "agentflow",
            "cache",
            "explain",
            "marker_demo.scan",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(step_explain.contains("Cache explain"));
        assert!(step_explain.contains("Flow: marker_demo.scan"));
        assert!(step_explain.contains("step:marker_demo/scan [hit]"));

        let cache_list = run(args(&[
            "agentflow",
            "cache",
            "list",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(cache_list.contains("\"schema_version\":\"agentflow.cache_entries.v0\""));
        assert!(cache_list.contains("\"tool_ref\":\"marker/marker_survival_scan\""));
        assert!(cache_list.contains("\"output_count\":1"));

        let naked_prune = run(args(&[
            "agentflow",
            "cache",
            "prune",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(naked_prune
            .message()
            .contains("cache prune requires --all or --older-than-seconds"));

        let old_prune = run(args(&[
            "agentflow",
            "cache",
            "prune",
            "--older-than-seconds",
            "31536000",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(old_prune.contains("\"removed_entries\":0"));

        let all_prune = run(args(&[
            "agentflow",
            "cache",
            "prune",
            "--all",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(all_prune.contains("\"removed_entries\":1"));

        let empty_cache = run(args(&[
            "agentflow",
            "cache",
            "list",
            "--json",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(empty_cache.contains("\"entries\":[]"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn report_command_generates_markdown_after_parsing() {
        let path = temp_project_path("report-markdown");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let tool_path = write_sample_tool(&path);
        run(args(&[
            "agentflow",
            "tools",
            "register",
            tool_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let artifact_path =
            write_sample_artifact(&path, "expression.tsv", "sample\tTP53\nA\t1.2\nB\t0.4\n");
        let import = run(args(&[
            "agentflow",
            "import",
            artifact_path.to_str().unwrap(),
            "--type",
            "TSV",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let artifact_id = import
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();
        let survival_path = write_sample_artifact(
            &path,
            "survival.tsv",
            "sample\ttime\tstatus\nA\t10\t1\nB\t12\t0\n",
        );
        let survival_import = run(args(&[
            "agentflow",
            "import",
            survival_path.to_str().unwrap(),
            "--type",
            "TSV",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        let survival_artifact_id = survival_import
            .split("Id: ")
            .nth(1)
            .and_then(|rest| rest.lines().next())
            .unwrap();
        let flow_path = write_sample_flow(&path, artifact_id, survival_artifact_id);
        run(args(&[
            "agentflow",
            "flow",
            "approve",
            flow_path.to_str().unwrap(),
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let report = run(args(&[
            "agentflow",
            "report",
            "marker_demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(report.contains("# Flow Report: Marker demo"));
        assert!(report.contains("`scan`"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn report_research_command_generates_project_research_markdown_after_parsing() {
        let path = temp_project_path("report-research-markdown");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let report = run(args(&[
            "agentflow",
            "report",
            "research",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(report.contains("# AgentFlow Research Report"));
        assert!(report.contains("No hypotheses recorded."));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn retry_command_reports_missing_step_after_parsing() {
        let path = temp_project_path("retry-missing-step");
        run(args(&[
            "agentflow",
            "init",
            "--name",
            "Demo",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap();

        let err = run(args(&[
            "agentflow",
            "retry",
            "scan",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(err.message().contains("not found: step scan"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn doctor_fails_outside_project() {
        let path = temp_project_path("doctor-missing");
        let err = run(args(&[
            "agentflow",
            "doctor",
            "--path",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(err.message().contains("not an AgentFlow project"));
    }
}
