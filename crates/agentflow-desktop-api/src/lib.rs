//! Read-only Rust facade over `agentflow-core`, for GUI frontends (currently
//! the `agentflow-desktop` Tauri app). No Tauri or other GUI-framework
//! dependency lives here, so this crate stays independently unit-testable
//! and reusable if a second frontend appears later.
//!
//! Nothing in this crate calls a mutating `ProjectStore` method. Run
//! triggering, tool registration, flow approval, etc. remain CLI-only for
//! now — see `docs/design/desktop-ui-design.md`.

pub use agentflow_core::storage::ProjectSummary;
use agentflow_core::storage::{ProjectStore, StorageError};

/// Aggregate read-only view of a project: its summary plus cheap counts.
/// `agentflow-core` has no single method returning this shape (flows in
/// particular have no `list` method, only lookup-by-known-id), so this is a
/// composition of several existing read calls.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProjectOverview {
    pub summary: ProjectSummary,
    pub flow_count: usize,
    pub tool_count: usize,
    pub run_count: usize,
    pub artifact_count: usize,
}

/// Open the AgentFlow project at `path` and return its overview.
pub fn open_project_overview(path: &str) -> Result<ProjectOverview, StorageError> {
    let store = ProjectStore::open(path)?;
    Ok(ProjectOverview {
        summary: store.summary()?,
        flow_count: store.count_flows()?,
        tool_count: store.list_tools()?.len(),
        run_count: store.list_runs(None)?.len(),
        artifact_count: store.list_artifacts()?.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-desktop-api-{test_name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn open_project_overview_reads_a_fresh_project_and_round_trips_through_json() {
        let path = temp_project_path("fresh");
        std::fs::create_dir_all(&path).unwrap();
        ProjectStore::init(&path, Some("test-project")).unwrap();

        let overview = open_project_overview(path.to_str().unwrap()).unwrap();

        assert_eq!(overview.summary.name, "test-project");
        assert_eq!(overview.flow_count, 0);
        assert_eq!(overview.tool_count, 0);
        assert_eq!(overview.run_count, 0);
        assert_eq!(overview.artifact_count, 0);

        // The main real risk in this crate: prove the Serialize derive chain
        // (ProjectOverview -> ProjectSummary -> PathBuf) actually works.
        let json = serde_json::to_string(&overview).unwrap();
        assert!(json.contains("\"name\":\"test-project\""));

        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn open_project_overview_surfaces_a_clear_error_for_a_non_project_directory() {
        let path = temp_project_path("not-a-project");
        std::fs::create_dir_all(&path).unwrap();

        let error = open_project_overview(path.to_str().unwrap()).unwrap_err();

        assert!(matches!(error, StorageError::NotProject(_)));
        assert!(error.to_string().contains("not an AgentFlow project"));

        let _ = std::fs::remove_dir_all(&path);
    }
}
