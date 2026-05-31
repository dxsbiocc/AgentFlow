//! Core AgentFlow runtime boundary.
//!
//! V0 deliberately exposes only a tiny surface. Runtime, storage, validation,
//! and reporting will grow behind this crate without depending on Omiga or UI
//! code.

pub mod argument;
pub mod branch;
pub mod comparison;
pub mod domain;
pub mod graph_patch;
pub mod handoff;
pub mod hypothesis;
pub mod observer;
pub mod report;
pub mod research;
pub mod runtime;
pub mod storage;
pub mod trace_guard;

pub use observer::ObservationRecord;

pub const ENGINE_NAME: &str = "agentflow";
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBoundary {
    pub core_depends_on_omiga: bool,
    pub core_depends_on_frontend: bool,
    pub schema_version: &'static str,
}

impl Default for RuntimeBoundary {
    fn default() -> Self {
        Self {
            core_depends_on_omiga: false,
            core_depends_on_frontend: false,
            schema_version: agentflow_schemas::STATUS_JSON_SCHEMA_V0,
        }
    }
}

pub fn version_line() -> String {
    format!("{ENGINE_NAME} {ENGINE_VERSION}")
}

pub fn runtime_boundary() -> RuntimeBoundary {
    RuntimeBoundary::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_uses_engine_name() {
        assert!(version_line().starts_with("agentflow "));
    }

    #[test]
    fn core_boundary_is_independent_of_omiga_and_frontend() {
        let boundary = runtime_boundary();
        assert!(!boundary.core_depends_on_omiga);
        assert!(!boundary.core_depends_on_frontend);
        assert_eq!(
            boundary.schema_version,
            agentflow_schemas::STATUS_JSON_SCHEMA_V0
        );
    }
}
