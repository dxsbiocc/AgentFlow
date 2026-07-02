// Hand-written mirrors of the Rust DTOs in `agentflow-desktop-api`
// (`ProjectSummary`/`ProjectOverview`). Two small structs don't justify a
// codegen dependency (ts-rs/specta) yet — revisit once the DTO surface grows
// in later slices.

export interface ProjectSummary {
  id: string;
  name: string;
  root_path: string;
  engine_version: string;
  created_at: number;
  updated_at: number;
}

export interface ProjectOverview {
  summary: ProjectSummary;
  flow_count: number;
  tool_count: number;
  run_count: number;
  artifact_count: number;
}
