use std::collections::BTreeSet;

use crate::branch::ProposedStep;
use crate::storage::{ExecutableToolSpec, ProjectStore, StorageError};

const SCORE_OUTPUT_TYPE: i32 = 10;
const SCORE_REQUIRED_INPUT: i32 = 3;
const SCORE_KEYWORD_NAME: i32 = 4;
const SCORE_KEYWORD_DESCRIPTION: i32 = 2;
const SCORE_MATURITY_VERIFIED: i32 = 3;
const SCORE_MATURITY_WRAPPED: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    High,
    Medium,
    Low,
}

impl Fit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CapabilityQuery {
    pub desired_output_type: Option<String>,
    pub available_input_types: Vec<String>,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCandidate {
    pub tool_ref: String,
    pub fit: Fit,
    pub score: i32,
    pub reason: String,
}

impl ProjectStore {
    pub fn match_tools(&self, query: &CapabilityQuery) -> Result<Vec<ToolCandidate>, StorageError> {
        let available_types = query
            .available_input_types
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let keywords = normalized_keywords(&query.keywords);
        let mut candidates = Vec::new();

        for summary in self.list_tools()? {
            let tool_ref = summary.tool_ref();
            let inspection = self.inspect_tool(&tool_ref)?;
            let description = extract_stored_string_field(&inspection.spec_json, "description")?;
            let executable = self.executable_tool(&tool_ref)?;
            let mut score = 0;
            let mut reasons = Vec::new();

            let output_match = query
                .desired_output_type
                .as_deref()
                .is_some_and(|desired| has_output_type(&executable, desired));
            if let Some(desired) = query.desired_output_type.as_deref() {
                if output_match {
                    score += SCORE_OUTPUT_TYPE;
                    reasons.push(format!("output:{desired}"));
                }
            }

            let required_inputs = executable
                .inputs
                .iter()
                .filter(|(_, input)| input.required)
                .collect::<Vec<_>>();
            let mut satisfied_required_inputs = 0usize;
            for (name, input) in &required_inputs {
                if available_types.contains(input.type_name.as_str()) {
                    satisfied_required_inputs += 1;
                    score += SCORE_REQUIRED_INPUT;
                    reasons.push(format!("input:{}:{}", name, input.type_name));
                }
            }

            let name_lower = summary.name.to_ascii_lowercase();
            let description_lower = description.to_ascii_lowercase();
            let mut name_keyword_hits = BTreeSet::new();
            let mut description_keyword_hits = BTreeSet::new();
            for keyword in &keywords {
                if name_lower.contains(keyword) {
                    name_keyword_hits.insert(keyword.as_str());
                    score += SCORE_KEYWORD_NAME;
                    reasons.push(format!("keyword:name:{keyword}"));
                }
                if description_lower.contains(keyword) {
                    description_keyword_hits.insert(keyword.as_str());
                    score += SCORE_KEYWORD_DESCRIPTION;
                    reasons.push(format!("keyword:description:{keyword}"));
                }
            }

            match summary.maturity.as_str() {
                "verified" => {
                    score += SCORE_MATURITY_VERIFIED;
                    reasons.push("maturity:verified".to_string());
                }
                "wrapped" => {
                    score += SCORE_MATURITY_WRAPPED;
                    reasons.push("maturity:wrapped".to_string());
                }
                _ => {
                    reasons.push(format!("maturity:{}", summary.maturity));
                }
            }

            let required_count = required_inputs.len();
            let all_required_inputs_satisfied = satisfied_required_inputs == required_count;
            let majority_required_inputs_satisfied =
                required_count > 0 && satisfied_required_inputs * 2 > required_count;
            let name_kw = name_keyword_hits.len();
            let desc_kw = description_keyword_hits.len();
            let strong_keyword_relevance = name_kw >= 1 || desc_kw >= 2;
            let fit = if output_match && all_required_inputs_satisfied {
                Fit::High
            } else if output_match || majority_required_inputs_satisfied {
                Fit::Medium
            } else if strong_keyword_relevance {
                reasons.push("relevance:keyword".to_string());
                Fit::Medium
            } else {
                Fit::Low
            };

            candidates.push(ToolCandidate {
                tool_ref,
                fit,
                score,
                reason: reason_text(reasons),
            });
        }

        candidates.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.tool_ref.cmp(&right.tool_ref))
        });
        Ok(candidates)
    }

    pub fn draft_step_for(
        &self,
        tool_ref: &str,
        available: &[(String, String)],
    ) -> Result<ProposedStep, StorageError> {
        let inspection = self.inspect_tool(tool_ref)?;
        let executable = self.executable_tool(tool_ref)?;
        let step_id = format!("step_{}", sanitize_step_id_part(&inspection.summary.name));

        let inputs = executable
            .inputs
            .iter()
            .filter(|(_, input)| input.required)
            .map(|(name, input)| {
                let artifact_id = available
                    .iter()
                    .find_map(|(type_name, artifact_id)| {
                        (type_name == &input.type_name).then(|| artifact_id.clone())
                    })
                    .unwrap_or_else(|| format!("artifact_REPLACE_{name}"));
                (name.clone(), artifact_id)
            })
            .collect();

        let params = executable
            .params
            .iter()
            .filter(|(_, param)| param.required)
            .map(|(name, _)| (name.clone(), format!("REPLACE_{name}")))
            .collect();

        let outputs = executable
            .outputs
            .keys()
            .map(|name| (name.clone(), format!("{step_id}_{name}")))
            .collect();

        Ok(ProposedStep {
            id: step_id,
            tool: tool_ref.to_string(),
            needs: Vec::new(),
            inputs,
            params,
            outputs,
        })
    }

    pub fn infer_step_needs(&self, step: &ProposedStep) -> Result<Vec<String>, StorageError> {
        let mut needs = BTreeSet::new();
        for (_, artifact_id) in &step.inputs {
            match self.inspect_artifact(artifact_id) {
                Ok(inspection) => {
                    if let Some(source_step_id) = inspection.summary.source_step_id {
                        needs.insert(source_step_id);
                    }
                }
                Err(StorageError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(needs.into_iter().collect())
    }
}

fn has_output_type(executable: &ExecutableToolSpec, desired: &str) -> bool {
    executable
        .outputs
        .values()
        .any(|output| output.type_name == desired)
}

fn normalized_keywords(keywords: &[String]) -> Vec<String> {
    keywords
        .iter()
        .map(|keyword| keyword.trim().to_ascii_lowercase())
        .filter(|keyword| !keyword.is_empty())
        .collect()
}

fn reason_text(reasons: Vec<String>) -> String {
    if reasons.is_empty() {
        "no match".to_string()
    } else {
        reasons.join(", ")
    }
}

fn sanitize_step_id_part(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "tool".to_string()
    } else {
        sanitized
    }
}

fn extract_stored_string_field(source: &str, field: &str) -> Result<String, StorageError> {
    let needle = format!("\"{field}\":\"");
    let start = source.find(&needle).ok_or_else(|| {
        StorageError::InvalidInput(format!("stored tool spec is missing {field}"))
    })? + needle.len();
    parse_json_string_tail(&source[start..])
}

fn parse_json_string_tail(source: &str) -> Result<String, StorageError> {
    let mut value = String::new();
    let mut chars = source.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Ok(value),
            '\\' => match chars.next() {
                Some('"') => value.push('"'),
                Some('\\') => value.push('\\'),
                Some('n') => value.push('\n'),
                Some('r') => value.push('\r'),
                Some('t') => value.push('\t'),
                Some(other) => value.push(other),
                None => {
                    return Err(StorageError::InvalidInput(
                        "unterminated stored tool spec string escape".to_string(),
                    ));
                }
            },
            other => value.push(other),
        }
    }
    Err(StorageError::InvalidInput(
        "unterminated stored tool spec string".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{
        ArtifactImportMode, ArtifactImportRequest, ComputedArtifactRequest, ToolSpec,
    };
    use std::fs;
    use std::path::PathBuf;

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-core-tool-select-{test_name}-{}-{}",
            std::process::id(),
            crate::storage::now_unix_seconds()
        ))
    }

    fn init_store(test_name: &str) -> (PathBuf, ProjectStore) {
        let path = temp_project_path(test_name);
        let store = ProjectStore::init(&path, Some("Tool Select Demo")).unwrap();
        (path, store)
    }

    fn register_tool(store: &ProjectStore, yaml: &str) {
        let spec = ToolSpec::from_simple_yaml(yaml).unwrap();
        store.register_tool(spec).unwrap();
    }

    fn tool_yaml(
        namespace: &str,
        name: &str,
        maturity: &str,
        description: &str,
        inputs: &str,
        params: &str,
        outputs: &str,
    ) -> String {
        format!(
            r#"
schema_version: agentflow.tool.v0
namespace: {namespace}
name: {name}
version: 0.1.0
maturity: {maturity}
description: {description}
inputs:
{inputs}
params:
{params}
outputs:
{outputs}
runtime:
  backend: local
  command:
    - /bin/echo
"#
        )
    }

    fn one_required_input(name: &str, type_name: &str) -> String {
        format!("  {name}:\n    type: {type_name}\n    required: true\n",)
    }

    fn no_inputs() -> &'static str {
        "  optional_context:\n    type: Context\n    required: false\n"
    }

    fn no_params() -> &'static str {
        "  threshold:\n    type: string\n    required: false\n"
    }

    fn markdown_output() -> &'static str {
        "  report:\n    type: Markdown\n"
    }

    fn write_input(path: &std::path::Path, name: &str) -> PathBuf {
        let file_path = path.join(name);
        fs::write(&file_path, "sample\tvalue\nA\t1\n").unwrap();
        file_path
    }

    fn computed_artifact(
        store: &ProjectStore,
        root: &std::path::Path,
        name: &str,
        source_step_id: &str,
    ) -> String {
        store
            .register_computed_artifact(ComputedArtifactRequest {
                source_path: write_input(root, name),
                artifact_type: "ExpressionTable".to_string(),
                output_name: "expression_table".to_string(),
                source_step_id: source_step_id.to_string(),
                source_run_id: "run_source".to_string(),
            })
            .unwrap()
            .summary
            .id
    }

    #[test]
    fn match_tools_scores_query_dimensions_and_assigns_fit() {
        let (path, store) = init_store("scores");
        register_tool(
            &store,
            &tool_yaml(
                "alpha",
                "marker_report",
                "verified",
                "Build marker report",
                &one_required_input("expression_table", "ExpressionTable"),
                no_params(),
                markdown_output(),
            ),
        );
        register_tool(
            &store,
            &tool_yaml(
                "beta",
                "survival_scan",
                "wrapped",
                "Scan marker survival table",
                "  expression_table:\n    type: ExpressionTable\n    required: true\n  survival_table:\n    type: SurvivalTable\n    required: true\n",
                no_params(),
                markdown_output(),
            ),
        );
        register_tool(
            &store,
            &tool_yaml(
                "gamma",
                "qc_table",
                "exploratory",
                "Quality control table",
                &one_required_input("counts", "RawCounts"),
                no_params(),
                "  table:\n    type: TSV\n",
            ),
        );

        let candidates = store
            .match_tools(&CapabilityQuery {
                desired_output_type: Some("Markdown".to_string()),
                available_input_types: vec!["ExpressionTable".to_string()],
                keywords: vec!["marker".to_string()],
            })
            .unwrap();

        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].tool_ref, "alpha/marker_report");
        assert_eq!(candidates[0].score, 22);
        assert_eq!(candidates[0].fit, Fit::High);
        assert!(candidates[0].reason.contains("output:Markdown"));
        assert!(candidates[0]
            .reason
            .contains("input:expression_table:ExpressionTable"));
        assert!(candidates[0].reason.contains("keyword:name:marker"));
        assert!(candidates[0].reason.contains("keyword:description:marker"));
        assert!(candidates[0].reason.contains("maturity:verified"));

        assert_eq!(candidates[1].tool_ref, "beta/survival_scan");
        assert_eq!(candidates[1].score, 16);
        assert_eq!(candidates[1].fit, Fit::Medium);
        assert_eq!(candidates[2].tool_ref, "gamma/qc_table");
        assert_eq!(candidates[2].score, 0);
        assert_eq!(candidates[2].fit, Fit::Low);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn match_tools_orders_score_descending_then_tool_ref() {
        let (path, store) = init_store("sort");
        for name in ["z_tool", "a_tool"] {
            register_tool(
                &store,
                &tool_yaml(
                    "tie",
                    name,
                    "exploratory",
                    "Same candidate",
                    no_inputs(),
                    no_params(),
                    markdown_output(),
                ),
            );
        }

        let candidates = store
            .match_tools(&CapabilityQuery {
                desired_output_type: Some("Markdown".to_string()),
                available_input_types: Vec::new(),
                keywords: Vec::new(),
            })
            .unwrap();

        assert_eq!(candidates[0].tool_ref, "tie/a_tool");
        assert_eq!(candidates[1].tool_ref, "tie/z_tool");
        assert_eq!(candidates[0].score, candidates[1].score);
        assert_eq!(candidates[0].fit, Fit::High);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn match_tools_promotes_name_keyword_relevance_without_io_match_to_medium() {
        let (path, store) = init_store("name-keyword-fit");
        register_tool(
            &store,
            &tool_yaml(
                "omics",
                "tcga_survival_scan",
                "exploratory",
                "Evaluate cohort statistics",
                no_inputs(),
                no_params(),
                markdown_output(),
            ),
        );

        let candidates = store
            .match_tools(&CapabilityQuery {
                desired_output_type: None,
                available_input_types: Vec::new(),
                keywords: vec!["THRSP".to_string(), "tcga".to_string()],
            })
            .unwrap();

        assert_eq!(candidates[0].tool_ref, "omics/tcga_survival_scan");
        assert_eq!(candidates[0].fit, Fit::Medium);
        assert!(candidates[0].reason.contains("keyword:name:tcga"));
        assert!(candidates[0].reason.contains("relevance:keyword"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn match_tools_keeps_single_description_keyword_without_io_match_low() {
        let (path, store) = init_store("single-description-keyword-fit");
        register_tool(
            &store,
            &tool_yaml(
                "omics",
                "cohort_scan",
                "exploratory",
                "Evaluate tcga cohort statistics",
                no_inputs(),
                no_params(),
                markdown_output(),
            ),
        );

        let candidates = store
            .match_tools(&CapabilityQuery {
                desired_output_type: None,
                available_input_types: Vec::new(),
                keywords: vec!["tcga".to_string()],
            })
            .unwrap();

        assert_eq!(candidates[0].tool_ref, "omics/cohort_scan");
        assert_eq!(candidates[0].fit, Fit::Low);
        assert!(candidates[0].reason.contains("keyword:description:tcga"));
        assert!(!candidates[0].reason.contains("relevance:keyword"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn match_tools_promotes_two_description_keywords_without_io_match_to_medium() {
        let (path, store) = init_store("two-description-keywords-fit");
        register_tool(
            &store,
            &tool_yaml(
                "omics",
                "cohort_scan",
                "exploratory",
                "Evaluate tcga survival statistics",
                no_inputs(),
                no_params(),
                markdown_output(),
            ),
        );

        let candidates = store
            .match_tools(&CapabilityQuery {
                desired_output_type: None,
                available_input_types: Vec::new(),
                keywords: vec!["tcga".to_string(), "survival".to_string()],
            })
            .unwrap();

        assert_eq!(candidates[0].tool_ref, "omics/cohort_scan");
        assert_eq!(candidates[0].fit, Fit::Medium);
        assert!(candidates[0].reason.contains("keyword:description:tcga"));
        assert!(candidates[0]
            .reason
            .contains("keyword:description:survival"));
        assert!(candidates[0].reason.contains("relevance:keyword"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn match_tools_keyword_relevance_without_io_match_never_promotes_to_high() {
        let (path, store) = init_store("keyword-fit-not-high");
        register_tool(
            &store,
            &tool_yaml(
                "omics",
                "tcga_survival_scan",
                "exploratory",
                "Evaluate tcga survival statistics",
                no_inputs(),
                no_params(),
                markdown_output(),
            ),
        );

        let candidates = store
            .match_tools(&CapabilityQuery {
                desired_output_type: None,
                available_input_types: Vec::new(),
                keywords: vec!["tcga".to_string(), "survival".to_string()],
            })
            .unwrap();

        assert_eq!(candidates[0].tool_ref, "omics/tcga_survival_scan");
        assert_eq!(candidates[0].fit, Fit::Medium);
        assert_ne!(candidates[0].fit, Fit::High);
        assert!(candidates[0].reason.contains("relevance:keyword"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn draft_step_for_maps_required_inputs_params_and_outputs() {
        let (path, store) = init_store("draft");
        register_tool(
            &store,
            &tool_yaml(
                "marker",
                "survival_scan",
                "wrapped",
                "Scan marker survival table",
                "  expression_table:\n    type: ExpressionTable\n    required: true\n  optional_notes:\n    type: Markdown\n    required: false\n  survival_table:\n    type: SurvivalTable\n    required: true\n",
                "  gene:\n    type: string\n    required: true\n  threshold:\n    type: string\n    required: false\n",
                "  report:\n    type: Markdown\n  table:\n    type: TSV\n",
            ),
        );

        let step = store
            .draft_step_for(
                "marker/survival_scan",
                &[(
                    "ExpressionTable".to_string(),
                    "artifact_expression".to_string(),
                )],
            )
            .unwrap();

        assert_eq!(step.id, "step_survival_scan");
        assert_eq!(step.tool, "marker/survival_scan");
        assert!(step.needs.is_empty());
        assert_eq!(
            step.inputs,
            vec![
                (
                    "expression_table".to_string(),
                    "artifact_expression".to_string()
                ),
                (
                    "survival_table".to_string(),
                    "artifact_REPLACE_survival_table".to_string()
                )
            ]
        );
        assert_eq!(
            step.params,
            vec![("gene".to_string(), "REPLACE_gene".to_string())]
        );
        assert_eq!(
            step.outputs,
            vec![
                (
                    "report".to_string(),
                    "step_survival_scan_report".to_string()
                ),
                ("table".to_string(), "step_survival_scan_table".to_string())
            ]
        );

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn draft_step_for_propagates_not_found() {
        let (path, store) = init_store("not-found");

        let error = store.draft_step_for("missing/tool", &[]).unwrap_err();

        assert!(matches!(error, StorageError::NotFound(_)));
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn infer_step_needs_collects_sorted_unique_computed_sources() {
        let (path, store) = init_store("infer-computed");
        let z_artifact = computed_artifact(&store, &path, "z.tsv", "step_z");
        let a_artifact = computed_artifact(&store, &path, "a.tsv", "step_a");
        let step = ProposedStep {
            id: "branch_step".to_string(),
            tool: "analysis/branch".to_string(),
            needs: Vec::new(),
            inputs: vec![
                ("z".to_string(), z_artifact.clone()),
                ("a".to_string(), a_artifact),
                ("z_again".to_string(), z_artifact),
            ],
            params: Vec::new(),
            outputs: Vec::new(),
        };

        let needs = store.infer_step_needs(&step).unwrap();

        assert_eq!(needs, vec!["step_a".to_string(), "step_z".to_string()]);
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn infer_step_needs_skips_imported_missing_and_placeholder_inputs() {
        let (path, store) = init_store("infer-skips");
        let imported = store
            .import_artifact(ArtifactImportRequest {
                source_path: write_input(&path, "imported.tsv"),
                artifact_type: "ExpressionTable".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap()
            .summary
            .id;
        let step = ProposedStep {
            id: "branch_step".to_string(),
            tool: "analysis/branch".to_string(),
            needs: Vec::new(),
            inputs: vec![
                ("imported".to_string(), imported),
                (
                    "placeholder".to_string(),
                    "artifact_REPLACE_expression_table".to_string(),
                ),
                ("missing".to_string(), "artifact_missing".to_string()),
            ],
            params: Vec::new(),
            outputs: Vec::new(),
        };

        let needs = store.infer_step_needs(&step).unwrap();

        assert!(needs.is_empty());
        let _ = fs::remove_dir_all(path);
    }
}
