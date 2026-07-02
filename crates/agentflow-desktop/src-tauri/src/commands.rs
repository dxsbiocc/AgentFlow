use agentflow_desktop_api::ProjectOverview;

/// Open an AgentFlow project directory and return its read-only overview.
///
/// This is the only Tauri IPC command in this slice — no mutating
/// `ProjectStore` call is ever exposed to the frontend. Errors are converted
/// to plain strings (`StorageError` already has clear `Display` messages,
/// e.g. "not an AgentFlow project: missing <path>/.agentflow/project.db") so
/// the frontend can render them directly without a second error-mapping layer.
#[tauri::command]
pub fn open_project(path: String) -> Result<ProjectOverview, String> {
    agentflow_desktop_api::open_project_overview(&path).map_err(|error| error.to_string())
}
