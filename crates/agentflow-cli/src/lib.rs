use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

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
    let mut args = args.into_iter();
    let _program = args.next();

    match next_arg(&mut args)? {
        None => Ok(usage()),
        Some(command) if matches!(command.as_str(), "--help" | "-h" | "help") => Ok(usage()),
        Some(command) if matches!(command.as_str(), "--version" | "-V" | "version") => {
            Ok(agentflow_core::version_line())
        }
        Some(command) if command == "init" => init_command(args),
        Some(command) if command == "status" => status_command(args),
        Some(command) if command == "doctor" => doctor_command(args),
        Some(command) if command == "tools" => tools_command(args),
        Some(command) if command == "import" => import_command(args),
        Some(command) if command == "artifacts" => artifacts_command(args),
        Some(command) if command == "flow" => flow_command(args),
        Some(command) if command == "run" => run_command(args),
        Some(command) if command == "run-step" => run_step_command(args),
        Some(command) if command == "report" => report_command(args),
        Some(command) if command == "cache" => cache_command(args),
        Some(command) if command == "retry" => retry_command(args),
        Some(command) if command == "observe" => observe_command(args),
        Some(command) if command == "observations" => observations_command(args),
        Some(command) if command == "research" => research_command(args),
        Some(command) if command == "patch" => patch_command(args),
        Some(command) if command == "compare" => compare_command(args),
        Some(command) if command == "logs" => logs_command(args),
        Some(command) => Err(CliError::UnknownCommand(command)),
    }
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
        "  agentflow import <file> --type <artifact-type> [--mode reference|copy] [--path <path>]",
        "  agentflow artifacts list [--json] [--path <path>]",
        "  agentflow artifacts inspect <artifact-id> [--json] [--path <path>]",
        "  agentflow flow validate <flow.yaml> [--json] [--path <path>]",
        "  agentflow flow approve <flow.yaml> [--path <path>]",
        "  agentflow flow inspect <flow-id> [--json] [--path <path>]",
        "  agentflow run <flow-id> [--path <path>]",
        "  agentflow run-step <step-id|flow.step|step:flow/step> [--path <path>]",
        "  agentflow report <flow-id> [--path <path>]",
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
        "  agentflow patch propose <flow-id> --title <text> --reason <text> (--patch-json <json>|--patch-file <file>) [--json] [--path <path>]",
        "  agentflow patch list <flow-id> [--json] [--path <path>]",
        "  agentflow patch approve <patch-id> [--json] [--path <path>]",
        "  agentflow patch reject <patch-id> --reason <text> [--json] [--path <path>]",
        "  agentflow patch apply <patch-id> [--json] [--path <path>]",
        "  agentflow compare steps <flow-id> --baseline <step-id> --candidate <step-id> --summary <text> [--winner baseline|candidate|tie|inconclusive] [--reason <text>] [--json] [--path <path>]",
        "  agentflow compare metrics <flow-id> --baseline <step-id> --candidate <step-id> --metric <name> [--direction higher|lower] [--json] [--path <path>]",
        "  agentflow compare list <flow-id> [--json] [--path <path>]",
        "  agentflow compare inspect <comparison-id> [--json] [--path <path>]",
        "  agentflow logs <run-or-attempt-id> [--path <path>]",
        "",
        "Implementation status:",
        "  V1 usable CLI runtime slice is available for approved executable flows.",
    ]
    .join("\n")
}

fn init_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_project_options(args, true)?;
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

fn status_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_project_options(args, false)?;
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

fn run_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "flow id", false)?;
    let flow_id = options
        .positional
        .ok_or_else(|| CliError::InvalidArgument("run requires <flow-id>".to_string()))?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
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

fn run_step_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "step id", false)?;
    let step_id = options
        .positional
        .ok_or_else(|| CliError::InvalidArgument("run-step requires <step-id>".to_string()))?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
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

fn logs_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "run or attempt id", false)?;
    let id = options.positional.ok_or_else(|| {
        CliError::InvalidArgument("logs requires <run-or-attempt-id>".to_string())
    })?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
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

fn report_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "flow id", false)?;
    let flow_id = options
        .positional
        .ok_or_else(|| CliError::InvalidArgument("report requires <flow-id>".to_string()))?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    store.generate_report_markdown(&flow_id).map_err(Into::into)
}

fn cache_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    match next_arg(&mut args)? {
        Some(command) if command == "explain" => cache_explain_command(args),
        Some(command) if command == "list" => cache_list_command(args),
        Some(command) if command == "prune" => cache_prune_command(args),
        Some(command) => Err(CliError::InvalidArgument(format!(
            "unknown cache command: {command}"
        ))),
        None => Err(CliError::InvalidArgument(
            "cache requires a command: explain, list, or prune".to_string(),
        )),
    }
}

fn cache_explain_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "flow id or step id", false)?;
    let target = options.positional.ok_or_else(|| {
        CliError::InvalidArgument("cache explain requires <flow-id|step-id>".to_string())
    })?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;

    let explanations = store.cache_explain_target(&target)?;
    Ok(format_cache_explanations(&target, &explanations))
}

fn cache_list_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_project_options(args, false)?;
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

fn cache_prune_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_cache_prune_options(args)?;
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

fn retry_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "step id", false)?;
    let step_id = options
        .positional
        .ok_or_else(|| CliError::InvalidArgument("retry requires <step-id>".to_string()))?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
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

fn observe_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_observe_options(args)?;
    let artifact_id = options
        .artifact_id
        .ok_or_else(|| CliError::InvalidArgument("observe requires <artifact-id>".to_string()))?;
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

fn observations_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    match next_arg(&mut args)? {
        Some(command) if command == "list" => observations_list_command(args),
        Some(command) if command == "inspect" => observations_inspect_command(args),
        Some(command) => Err(CliError::InvalidArgument(format!(
            "unknown observations command: {command}"
        ))),
        None => Err(CliError::InvalidArgument(
            "observations requires a command: list or inspect".to_string(),
        )),
    }
}

fn observations_list_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_project_options(args, false)?;
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

fn observations_inspect_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "observation id", true)?;
    let observation_id = options.positional.ok_or_else(|| {
        CliError::InvalidArgument("observations inspect requires <observation-id>".to_string())
    })?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let observation = store.inspect_observation(&observation_id)?;

    if options.project.json {
        Ok(observation_json(&observation))
    } else {
        Ok(format_observation(&observation))
    }
}

fn research_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    match next_arg(&mut args)? {
        Some(command) if command == "note" => research_note_command(args),
        Some(command) if command == "list" => research_list_command(args),
        Some(command) if command == "inspect" => research_inspect_command(args),
        Some(command) => Err(CliError::InvalidArgument(format!(
            "unknown research command: {command}"
        ))),
        None => Err(CliError::InvalidArgument(
            "research requires a command: note, list, or inspect".to_string(),
        )),
    }
}

fn research_note_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_research_note_options(args)?;
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

fn research_list_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_project_options(args, false)?;
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

fn research_inspect_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "research note id", true)?;
    let note_id = options.positional.ok_or_else(|| {
        CliError::InvalidArgument("research inspect requires <note-id>".to_string())
    })?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let note = store.inspect_research_note(&note_id)?;
    if options.project.json {
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

fn patch_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    match next_arg(&mut args)? {
        Some(command) if command == "propose" => patch_propose_command(args),
        Some(command) if command == "list" => patch_list_command(args),
        Some(command) if command == "approve" => patch_approve_command(args),
        Some(command) if command == "reject" => patch_reject_command(args),
        Some(command) if command == "apply" => patch_apply_command(args),
        Some(command) => Err(CliError::InvalidArgument(format!(
            "unknown patch command: {command}"
        ))),
        None => Err(CliError::InvalidArgument(
            "patch requires a command: propose, list, approve, reject, or apply".to_string(),
        )),
    }
}

fn patch_propose_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_graph_patch_propose_options(args)?;
    let flow_id = options
        .flow_id
        .ok_or_else(|| CliError::InvalidArgument("patch propose requires <flow-id>".to_string()))?;
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

fn patch_list_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "flow id", true)?;
    let flow_id = options
        .positional
        .ok_or_else(|| CliError::InvalidArgument("patch list requires <flow-id>".to_string()))?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let patches = store.list_graph_patches(&flow_id)?;

    if options.project.json {
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

fn patch_approve_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "patch id", true)?;
    let patch_id = options.positional.ok_or_else(|| {
        CliError::InvalidArgument("patch approve requires <patch-id>".to_string())
    })?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let patch = store.approve_graph_patch(&patch_id)?;

    if options.project.json {
        Ok(graph_patch_json(&patch))
    } else {
        Ok(format_graph_patch("Approved graph patch", &patch))
    }
}

fn patch_reject_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_graph_patch_reject_options(args)?;
    let patch_id = options
        .patch_id
        .ok_or_else(|| CliError::InvalidArgument("patch reject requires <patch-id>".to_string()))?;
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

fn patch_apply_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "patch id", true)?;
    let patch_id = options
        .positional
        .ok_or_else(|| CliError::InvalidArgument("patch apply requires <patch-id>".to_string()))?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let application = store.apply_graph_patch(&patch_id)?;

    if options.project.json {
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

fn compare_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    match next_arg(&mut args)? {
        Some(command) if command == "steps" => compare_steps_command(args),
        Some(command) if command == "metrics" => compare_metrics_command(args),
        Some(command) if command == "list" => compare_list_command(args),
        Some(command) if command == "inspect" => compare_inspect_command(args),
        Some(command) => Err(CliError::InvalidArgument(format!(
            "unknown compare command: {command}"
        ))),
        None => Err(CliError::InvalidArgument(
            "compare requires a command: steps, metrics, list, or inspect".to_string(),
        )),
    }
}

fn compare_steps_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_compare_steps_options(args)?;
    let flow_id = options
        .flow_id
        .ok_or_else(|| CliError::InvalidArgument("compare steps requires <flow-id>".to_string()))?;
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

fn compare_metrics_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_compare_metrics_options(args)?;
    let flow_id = options.flow_id.ok_or_else(|| {
        CliError::InvalidArgument("compare metrics requires <flow-id>".to_string())
    })?;
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

fn compare_list_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "flow id", true)?;
    let flow_id = options
        .positional
        .ok_or_else(|| CliError::InvalidArgument("compare list requires <flow-id>".to_string()))?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let comparisons = store.list_branch_comparisons(&flow_id)?;

    if options.project.json {
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

fn compare_inspect_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "comparison id", true)?;
    let comparison_id = options.positional.ok_or_else(|| {
        CliError::InvalidArgument("compare inspect requires <comparison-id>".to_string())
    })?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let comparison = store.inspect_branch_comparison(&comparison_id)?;

    if options.project.json {
        Ok(comparison.to_json())
    } else {
        Ok(format_branch_comparison("Branch comparison", &comparison))
    }
}

fn doctor_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_project_options(args, false)?;
    let path = options.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&path)?;
    let migrations = store.applied_migrations()?;
    Ok(format!(
        "AgentFlow project ok\nPath: {}\nApplied migrations: {}",
        store.root_path().display(),
        migrations.len()
    ))
}

fn tools_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    match next_arg(&mut args)? {
        Some(command) if command == "register" => tools_register_command(args),
        Some(command) if command == "list" => tools_list_command(args),
        Some(command) if command == "inspect" => tools_inspect_command(args),
        Some(command) => Err(CliError::InvalidArgument(format!(
            "unknown tools command: {command}"
        ))),
        None => Err(CliError::InvalidArgument(
            "tools requires a command: register, list, or inspect".to_string(),
        )),
    }
}

fn tools_register_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "tool spec path", false)?;
    let spec_path = options.positional.ok_or_else(|| {
        CliError::InvalidArgument("tools register requires <tool.yaml>".to_string())
    })?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
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

    Ok(spec)
}

fn tools_list_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_project_options(args, false)?;
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

fn tools_inspect_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "tool ref", true)?;
    let tool_ref = options.positional.ok_or_else(|| {
        CliError::InvalidArgument("tools inspect requires <tool-ref>".to_string())
    })?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let inspection = store.inspect_tool(&tool_ref)?;

    if options.project.json {
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

fn import_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_import_options(args)?;
    let source_path = options.source_path.ok_or_else(|| {
        CliError::InvalidArgument("import requires <file> before options".to_string())
    })?;
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

fn artifacts_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    match next_arg(&mut args)? {
        Some(command) if command == "list" => artifacts_list_command(args),
        Some(command) if command == "inspect" => artifacts_inspect_command(args),
        Some(command) => Err(CliError::InvalidArgument(format!(
            "unknown artifacts command: {command}"
        ))),
        None => Err(CliError::InvalidArgument(
            "artifacts requires a command: list or inspect".to_string(),
        )),
    }
}

fn artifacts_list_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_project_options(args, false)?;
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

fn artifacts_inspect_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "artifact id", true)?;
    let artifact_id = options.positional.ok_or_else(|| {
        CliError::InvalidArgument("artifacts inspect requires <artifact-id>".to_string())
    })?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let inspection = store.inspect_artifact(&artifact_id)?;

    if options.project.json {
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

fn flow_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    match next_arg(&mut args)? {
        Some(command) if command == "validate" => flow_validate_command(args),
        Some(command) if command == "approve" => flow_approve_command(args),
        Some(command) if command == "inspect" => flow_inspect_command(args),
        Some(command) => Err(CliError::InvalidArgument(format!(
            "unknown flow command: {command}"
        ))),
        None => Err(CliError::InvalidArgument(
            "flow requires a command: validate, approve, or inspect".to_string(),
        )),
    }
}

fn flow_validate_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "flow spec path", true)?;
    let flow_path = options.positional.ok_or_else(|| {
        CliError::InvalidArgument("flow validate requires <flow.yaml>".to_string())
    })?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
    let source = fs::read_to_string(&flow_path)?;
    let draft = agentflow_core::storage::FlowDraft::from_simple_yaml(&source)?;
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let report = store.validate_flow(&draft);

    if options.project.json {
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

fn flow_approve_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "flow spec path", false)?;
    let flow_path = options.positional.ok_or_else(|| {
        CliError::InvalidArgument("flow approve requires <flow.yaml>".to_string())
    })?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
    let source = fs::read_to_string(&flow_path)?;
    let draft = agentflow_core::storage::FlowDraft::from_simple_yaml(&source)?;
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let approval = store.approve_flow(draft, Some(PathBuf::from(&flow_path).as_path()))?;

    Ok(format!(
        "Approved flow\nId: {}\nName: {}\nSteps: {}\nEdges: {}",
        approval.flow_id, approval.name, approval.step_count, approval.edge_count
    ))
}

fn flow_inspect_command<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_single_positional_options(args, "flow id", true)?;
    let flow_id = options
        .positional
        .ok_or_else(|| CliError::InvalidArgument("flow inspect requires <flow-id>".to_string()))?;
    let project_path = options.project.path.unwrap_or(std::env::current_dir()?);
    let store = agentflow_core::storage::ProjectStore::open(&project_path)?;
    let inspection = store.inspect_flow(&flow_id)?;

    if options.project.json {
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
struct SinglePositionalOptions {
    project: ProjectOptions,
    positional: Option<String>,
}

#[derive(Debug, Default)]
struct CachePruneOptions {
    project: ProjectOptions,
    all: bool,
    older_than_seconds: Option<u64>,
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

fn parse_import_options<I>(args: I) -> Result<ImportOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = ImportOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project_path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--type" => {
                options.artifact_type = Some(require_value("--type", &mut args)?);
            }
            "--mode" => {
                let mode = require_value("--mode", &mut args)?;
                options.mode = agentflow_core::storage::ArtifactImportMode::parse(&mode)
                    .ok_or_else(|| {
                        CliError::InvalidArgument("--mode must be reference or copy".to_string())
                    })?;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                if options.source_path.is_some() {
                    return Err(CliError::InvalidArgument(format!(
                        "expected one file to import, got extra argument: {arg}"
                    )));
                }
                options.source_path = Some(arg);
            }
        }
    }

    Ok(options)
}

fn parse_observe_options<I>(args: I) -> Result<ObserveOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = ObserveOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project_path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--adapter" => {
                options.adapter = Some(require_value("--adapter", &mut args)?);
            }
            "--json" => {
                options.json = true;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                if options.artifact_id.is_some() {
                    return Err(CliError::InvalidArgument(format!(
                        "expected one artifact id, got extra argument: {arg}"
                    )));
                }
                options.artifact_id = Some(arg);
            }
        }
    }

    Ok(options)
}

fn parse_graph_patch_propose_options<I>(args: I) -> Result<GraphPatchProposeOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = GraphPatchProposeOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project_path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--title" => {
                options.title = Some(require_value("--title", &mut args)?);
            }
            "--reason" => {
                options.reason = Some(require_value("--reason", &mut args)?);
            }
            "--patch-json" => {
                options.patch_json = Some(require_value("--patch-json", &mut args)?);
            }
            "--patch-file" => {
                options.patch_file = Some(PathBuf::from(require_value("--patch-file", &mut args)?));
            }
            "--json" => {
                options.json = true;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                if options.flow_id.is_some() {
                    return Err(CliError::InvalidArgument(format!(
                        "expected one flow id, got extra argument: {arg}"
                    )));
                }
                options.flow_id = Some(arg);
            }
        }
    }

    Ok(options)
}

fn parse_graph_patch_reject_options<I>(args: I) -> Result<GraphPatchRejectOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = GraphPatchRejectOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project_path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--reason" => {
                options.reason = Some(require_value("--reason", &mut args)?);
            }
            "--json" => {
                options.json = true;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                if options.patch_id.is_some() {
                    return Err(CliError::InvalidArgument(format!(
                        "expected one patch id, got extra argument: {arg}"
                    )));
                }
                options.patch_id = Some(arg);
            }
        }
    }

    Ok(options)
}

fn parse_compare_steps_options<I>(args: I) -> Result<CompareStepsOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = CompareStepsOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project_path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--baseline" => {
                options.baseline_step = Some(require_value("--baseline", &mut args)?);
            }
            "--candidate" => {
                options.candidate_step = Some(require_value("--candidate", &mut args)?);
            }
            "--summary" => {
                options.summary = Some(require_value("--summary", &mut args)?);
            }
            "--winner" => {
                options.winner = Some(require_value("--winner", &mut args)?);
            }
            "--reason" => {
                options.reason = Some(require_value("--reason", &mut args)?);
            }
            "--json" => {
                options.json = true;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                if options.flow_id.is_some() {
                    return Err(CliError::InvalidArgument(format!(
                        "expected one flow id, got extra argument: {arg}"
                    )));
                }
                options.flow_id = Some(arg);
            }
        }
    }

    Ok(options)
}

fn parse_compare_metrics_options<I>(args: I) -> Result<CompareMetricsOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = CompareMetricsOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project_path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--baseline" => {
                options.baseline_step = Some(require_value("--baseline", &mut args)?);
            }
            "--candidate" => {
                options.candidate_step = Some(require_value("--candidate", &mut args)?);
            }
            "--metric" => {
                options.metric = Some(require_value("--metric", &mut args)?);
            }
            "--direction" => {
                options.direction = Some(require_value("--direction", &mut args)?);
            }
            "--json" => {
                options.json = true;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                if options.flow_id.is_some() {
                    return Err(CliError::InvalidArgument(format!(
                        "expected one flow id, got extra argument: {arg}"
                    )));
                }
                options.flow_id = Some(arg);
            }
        }
    }

    Ok(options)
}

fn parse_research_note_options<I>(args: I) -> Result<ResearchNoteOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = ResearchNoteOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project_path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--problem" => {
                options.problem = Some(require_value("--problem", &mut args)?);
            }
            "--question" => {
                options.question = Some(require_value("--question", &mut args)?);
            }
            "--finding" => {
                options.finding = Some(require_value("--finding", &mut args)?);
            }
            "--confidence" => {
                options.confidence = Some(require_value("--confidence", &mut args)?);
            }
            "--source" => {
                options.source = Some(require_value("--source", &mut args)?);
            }
            _ => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
        }
    }

    Ok(options)
}

fn parse_single_positional_options<I>(
    args: I,
    label: &str,
    allow_json: bool,
) -> Result<SinglePositionalOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = SinglePositionalOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project.path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--json" if allow_json => {
                options.project.json = true;
            }
            "--json" => {
                return Err(CliError::InvalidArgument(
                    "--json is not valid for this command".to_string(),
                ));
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                if options.positional.is_some() {
                    return Err(CliError::InvalidArgument(format!(
                        "expected one {label}, got extra argument: {arg}"
                    )));
                }
                options.positional = Some(arg);
            }
        }
    }

    Ok(options)
}

fn parse_cache_prune_options<I>(args: I) -> Result<CachePruneOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = CachePruneOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--path" => {
                options.project.path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--json" => {
                options.project.json = true;
            }
            "--all" => {
                options.all = true;
            }
            "--older-than-seconds" => {
                let value = require_value("--older-than-seconds", &mut args)?;
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
                options.older_than_seconds = Some(seconds);
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
            _ => {
                return Err(CliError::InvalidArgument(format!(
                    "cache prune does not accept positional argument: {arg}"
                )));
            }
        }
    }

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

fn parse_project_options<I>(args: I, allow_name: bool) -> Result<ProjectOptions, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = ProjectOptions::default();
    let mut args = args.into_iter();

    while let Some(arg) = next_arg(&mut args)? {
        match arg.as_str() {
            "--name" if allow_name => {
                options.name = Some(require_value("--name", &mut args)?);
            }
            "--name" => {
                return Err(CliError::InvalidArgument(
                    "--name is only valid for init".to_string(),
                ));
            }
            "--path" => {
                options.path = Some(PathBuf::from(require_value("--path", &mut args)?));
            }
            "--json" => {
                options.json = true;
            }
            _ => {
                return Err(CliError::InvalidArgument(format!("unknown option: {arg}")));
            }
        }
    }

    Ok(options)
}

fn require_value<I>(flag: &str, args: &mut I) -> Result<String, CliError>
where
    I: Iterator<Item = OsString>,
{
    next_arg(args)?.ok_or_else(|| CliError::InvalidArgument(format!("{flag} requires a value")))
}

fn next_arg<I>(args: &mut I) -> Result<Option<String>, CliError>
where
    I: Iterator<Item = OsString>,
{
    args.next()
        .map(|arg| {
            arg.into_string()
                .map_err(|_| CliError::InvalidArgument("argument is not valid UTF-8".to_string()))
        })
        .transpose()
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

fn research_notes_json(notes: &[agentflow_core::research::ResearchNote]) -> String {
    let items = notes
        .iter()
        .map(agentflow_core::research::ResearchNote::to_json)
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"schema_version\":\"agentflow.research_notes.v0\",\"notes\":[{items}]}}")
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

fn format_attempts(attempts: &[agentflow_core::runtime::AttemptSummary]) -> String {
    if attempts.is_empty() {
        return "_none_".to_string();
    }

    attempts
        .iter()
        .map(|attempt| {
            format!(
                "{} {} [{}] {}",
                attempt.attempt_id,
                attempt.step_id,
                attempt.status,
                attempt.workdir.display()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    min_rows: 1
  survival_table:
    type: TSV
    required: true
    required_columns: sample,time,status
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
    min_rows: 1
  survival_table:
    type: TSV
    required: true
    required_columns: sample,time,status
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

        let survival_path =
            write_sample_artifact(path, "survival.tsv", "sample\ttime\tstatus\nA\t10\t1\n");
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
    }

    #[test]
    fn unknown_command_is_error() {
        let err = run(args(&["agentflow", "unknown"])).unwrap_err();
        assert!(err.message().contains("unknown command: unknown"));
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
