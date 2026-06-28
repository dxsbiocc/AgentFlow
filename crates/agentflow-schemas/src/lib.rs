//! Versioned public schema identifiers for AgentFlow contracts.
//!
//! V0 keeps schemas centralized so CLI, core, and future integrations do not
//! invent incompatible contract names while the runtime is still small.

pub const TOOL_SCHEMA_V0: &str = "agentflow.tool.v0";
pub const TOOL_LIST_JSON_SCHEMA_V0: &str = "agentflow.tool_list.v0";
pub const TOOL_INSPECTION_JSON_SCHEMA_V0: &str = "agentflow.tool_inspection.v0";
pub const ARTIFACT_LIST_JSON_SCHEMA_V0: &str = "agentflow.artifact_list.v0";
pub const ARTIFACT_INSPECTION_JSON_SCHEMA_V0: &str = "agentflow.artifact_inspection.v0";
pub const FLOW_SCHEMA_V0: &str = "agentflow.flow.v0";
pub const MODULE_SCHEMA_V0: &str = "agentflow.module.v0";
pub const FLOW_VALIDATION_JSON_SCHEMA_V0: &str = "agentflow.flow_validation.v0";
pub const FLOW_INSPECTION_JSON_SCHEMA_V0: &str = "agentflow.flow_inspection.v0";
pub const STATUS_JSON_SCHEMA_V0: &str = "agentflow.status.v0";
pub const REPORT_MANIFEST_SCHEMA_V0: &str = "agentflow.report_manifest.v0";

pub const V0_TABLES: &[&str] = &[
    "schema_migrations",
    "projects",
    "flows",
    "steps",
    "edges",
    "tools",
    "tool_versions",
    "artifacts",
    "runs",
    "run_attempts",
    "cache_entries",
    "observations",
    "events",
    "reports",
];

pub const DEFERRED_TABLES: &[&str] = &[
    "tool_catalog_entries",
    "tool_capabilities",
    "hypotheses",
    "research_notes",
    "research_sources",
    "research_queries",
    "research_hits",
    "research_documents",
    "citations",
    "evidence_claims",
    "approvals",
];

pub fn known_schemas() -> &'static [&'static str] {
    &[
        TOOL_SCHEMA_V0,
        TOOL_LIST_JSON_SCHEMA_V0,
        TOOL_INSPECTION_JSON_SCHEMA_V0,
        ARTIFACT_LIST_JSON_SCHEMA_V0,
        ARTIFACT_INSPECTION_JSON_SCHEMA_V0,
        FLOW_SCHEMA_V0,
        MODULE_SCHEMA_V0,
        FLOW_VALIDATION_JSON_SCHEMA_V0,
        FLOW_INSPECTION_JSON_SCHEMA_V0,
        STATUS_JSON_SCHEMA_V0,
        REPORT_MANIFEST_SCHEMA_V0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_names_are_versioned() {
        for schema in known_schemas() {
            assert!(schema.starts_with("agentflow."));
            assert!(schema.ends_with(".v0"));
        }
    }

    #[test]
    fn v0_tables_match_runtime_mvp_scope() {
        assert_eq!(
            V0_TABLES,
            [
                "schema_migrations",
                "projects",
                "flows",
                "steps",
                "edges",
                "tools",
                "tool_versions",
                "artifacts",
                "runs",
                "run_attempts",
                "cache_entries",
                "observations",
                "events",
                "reports",
            ]
        );
    }

    #[test]
    fn deferred_tables_are_not_in_v0_tables() {
        for deferred in DEFERRED_TABLES {
            assert!(
                !V0_TABLES.contains(deferred),
                "deferred table {deferred} leaked into V0"
            );
        }
    }
}
