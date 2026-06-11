use std::collections::HashMap;
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::str::FromStr;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use agentflow_core::domain::ToolMaturity;
use agentflow_core::storage::{ProjectStore, ToolSpec};

use crate::cli_args::SynthArgs;
use crate::{last_value, CliError};

pub(crate) const DEFAULT_SYNTHESIZER: &str = "claude -p";
const SYNTH_VERSION: &str = "0.1.0";
const VALIDATION_TIMEOUT: Duration = Duration::from_secs(60);
const CBIOPORTAL_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);
const SOURCE_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);
const CBIOPORTAL_API_BASE: &str = "https://www.cbioportal.org/api";
const CBIOPORTAL_CLIENT_RELATIVE_DIR: &str = "examples/tools";
const MAX_AUTO_SYNTH_ATTEMPTS: usize = 3;
const VALIDATION_PATH: &str = "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin";
const PRIMARY_VALIDATION_GENE: &str = "TP53";
const ALTERNATE_VALIDATION_GENE: &str = "EGFR";
const MAX_SOURCE_PROBE_BYTES: usize = 65_536;
const MAX_PROBED_SOURCE_CANDIDATES: usize = 5;
const PUBLIC_SOURCE_ALLOWLIST: &[&str] = &[
    "www.cbioportal.org",
    "eutils.ncbi.nlm.nih.gov",
    "www.ncbi.nlm.nih.gov",
    "www.ebi.ac.uk",
    "rest.ensembl.org",
    "api.gdc.cancer.gov",
];
const CBIOPORTAL_DISCOVERY_FETCH_PY: &str = r#"import json
import sys
import urllib.request

url = sys.argv[1]
timeout = float(sys.argv[2])
request = urllib.request.Request(url, headers={"Accept": "application/json"})

# SSRF defense: do not follow redirects after Rust host allowlist checks.
class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None

_opener = urllib.request.build_opener(_NoRedirect)
with _opener.open(request, timeout=timeout) as response:
    body = response.read().decode("utf-8")
json.loads(body)
print(body, end="" if body.endswith("\n") else "\n")
"#;
const SOURCE_PROBE_FETCH_PY: &str = r#"import sys
import urllib.request

url = sys.argv[1]
timeout = float(sys.argv[2])
limit = int(sys.argv[3])
request = urllib.request.Request(
    url,
    headers={"Accept": "application/json, text/plain, text/html, */*"},
)

# SSRF defense: do not follow redirects after Rust host allowlist checks.
class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None

_opener = urllib.request.build_opener(_NoRedirect)
with _opener.open(request, timeout=timeout) as response:
    body = response.read(limit)
text = body.decode("utf-8", errors="replace")
print(text, end="" if text.endswith("\n") else "\n")
"#;
const DEFAULT_SYNTH_DOMAIN_PARAMS: &[SynthDomainParam] = &[SynthDomainParam {
    name: "gene",
    type_name: "string",
    required: true,
}];

type JsonObject = HashMap<String, String>;

#[derive(Debug, Clone, Copy)]
struct SynthDomainParam {
    name: &'static str,
    type_name: &'static str,
    required: bool,
}

#[derive(Debug, Default)]
struct SynthOptions {
    name: Option<String>,
    description: Option<String>,
    fixture: Option<PathBuf>,
    expect: Option<String>,
    synthesizer: Option<String>,
    path: Option<PathBuf>,
}

#[derive(Debug)]
struct ValidationOutput {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    timed_out: bool,
    result_output: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct SynthValidationInputs<'a> {
    gene: &'a str,
}

#[derive(Debug)]
struct AutoSynthCandidate {
    script: String,
    fixture: String,
    alternate_fixture: String,
    expect: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublicSourceCandidate {
    name: String,
    base_url: String,
    probe_url: String,
    access_note: String,
    required_data: String,
    has_required_data: String,
    required_data_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ViablePublicSource {
    candidate: PublicSourceCandidate,
    probe_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceDiscoveryReport {
    candidates: Vec<PublicSourceCandidate>,
    viable: Vec<ViablePublicSource>,
    data_requirements: String,
    trace: String,
    proposal_was_json: bool,
}

impl SourceDiscoveryReport {
    fn first_viable(&self) -> Option<&ViablePublicSource> {
        self.viable.first()
    }

    fn should_enforce_research_gap(&self) -> bool {
        self.proposal_was_json || !self.candidates.is_empty()
    }
}

#[derive(Debug)]
pub(crate) enum AutoSynthToolResult {
    Registered(String),
    Rejected(String),
    RegisteredWithSource {
        tool_ref: String,
        source_trace: String,
    },
    RejectedWithSource {
        reason: String,
        source_trace: String,
        research_gap: bool,
    },
}

pub(crate) fn synth_command(args: SynthArgs) -> Result<String, CliError> {
    let options = SynthOptions {
        name: last_value(args.name),
        description: last_value(args.description),
        fixture: last_value(args.fixture),
        expect: last_value(args.expect),
        synthesizer: last_value(args.synthesizer),
        path: last_value(args.project.path),
    };
    run_synth(options)
}

fn run_synth(options: SynthOptions) -> Result<String, CliError> {
    let name = require_option(options.name, "--name")?;
    validate_tool_name(&name)?;
    let description = require_option(options.description, "--description")?;
    let fixture = require_option(options.fixture, "--fixture")?;
    let fixture = fs::canonicalize(&fixture).map_err(|error| {
        CliError::Core(format!(
            "failed to resolve fixture {}: {error}",
            fixture.display()
        ))
    })?;
    let expect = require_option(options.expect, "--expect")?;
    let project_path = options.path.unwrap_or(std::env::current_dir()?);
    let store = ProjectStore::open(&project_path)?;
    let script_path = synth_script_path(store.root_path(), &name);

    let prompt = build_synth_prompt(&description);
    let synthesizer = configured_or_default_synthesizer(store.root_path(), options.synthesizer)?;
    let candidate = run_project_synthesizer(store.root_path(), &synthesizer, &prompt)?;
    let script = strip_markdown_fence(&candidate);
    if script.trim().is_empty() {
        return Err(CliError::Core(
            "synthesizer produced an empty candidate script".to_string(),
        ));
    }

    if let Some(parent) = script_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&script_path, script.as_bytes())?;
    let script_path = fs::canonicalize(&script_path)?;

    let validation = validate_candidate_script(&script_path, &fixture)?;
    if validation.timed_out {
        return Err(CliError::Core(format!(
            "candidate script timed out after {}s\nScript: {}\nStdout:\n{}\nStderr:\n{}",
            VALIDATION_TIMEOUT.as_secs(),
            script_path.display(),
            validation.stdout,
            validation.stderr
        )));
    }
    if validation.exit_code != Some(0) {
        return Err(CliError::Core(format!(
            "candidate script failed with exit code {}\nScript: {}\nStdout:\n{}\nStderr:\n{}",
            validation
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            script_path.display(),
            validation.stdout,
            validation.stderr
        )));
    }
    if !validation.stdout.contains(&expect) {
        return Ok(format!(
            concat!(
                "REJECTED\n",
                "Script: {}\n",
                "Expected substring: {}\n",
                "Stdout:\n{}\n",
                "Stderr:\n{}"
            ),
            script_path.display(),
            expect,
            validation.stdout,
            validation.stderr
        ));
    }

    let spec_yaml = synthesized_tool_yaml(&name, &description, &script_path);
    let spec = ToolSpec::from_simple_yaml(&spec_yaml)?;
    let registration = store.register_tool(spec)?;
    Ok(format!(
        concat!(
            "VALIDATED -> registered as exploratory (low trust)\n",
            "Tool: {}\n",
            "Version: {}\n",
            "Script: {}\n",
            "Spec hash: {}"
        ),
        registration.tool_ref,
        registration.version,
        script_path.display(),
        registration.spec_hash
    ))
}

pub(crate) fn auto_synthesize_agent_tool(
    store: &ProjectStore,
    synthesizer: &str,
    hypothesis_statement: &str,
    capability_need: &str,
    representative_gene: Option<&str>,
) -> Result<AutoSynthToolResult, CliError> {
    auto_synthesize_agent_tool_with_fetcher(
        store,
        synthesizer,
        hypothesis_statement,
        capability_need,
        representative_gene,
        fetch_public_source_probe_with_python,
    )
}

fn auto_synthesize_agent_tool_with_fetcher<F>(
    store: &ProjectStore,
    synthesizer: &str,
    hypothesis_statement: &str,
    capability_need: &str,
    representative_gene: Option<&str>,
    fetch_probe: F,
) -> Result<AutoSynthToolResult, CliError>
where
    F: FnMut(&str, Duration) -> Option<String>,
{
    let discovery = discover_public_sources_with_fetcher(
        store.root_path(),
        synthesizer,
        hypothesis_statement,
        capability_need,
        fetch_probe,
    )?;
    let (base_prompt, source_trace, research_gap_on_failure) = if discovery
        .should_enforce_research_gap()
    {
        let Some(viable) = discovery.first_viable() else {
            let reason = no_viable_public_source_reason_for(&discovery.data_requirements);
            return Ok(AutoSynthToolResult::RejectedWithSource {
                reason,
                source_trace: discovery.trace,
                research_gap: true,
            });
        };
        let trace = discovery.trace.clone();
        let cbioportal_grounding = if is_cbioportal_source(viable) {
            discover_cbioportal_grounding(hypothesis_statement)
        } else {
            None
        };
        (
            build_prompt_for_viable_source(
                hypothesis_statement,
                capability_need,
                viable,
                &trace,
                cbioportal_grounding.as_deref(),
            ),
            Some(trace),
            true,
        )
    } else {
        let grounding = discover_cbioportal_grounding(hypothesis_statement);
        (
            build_auto_synth_prompt(hypothesis_statement, capability_need, grounding.as_deref()),
            None,
            false,
        )
    };
    let runtime_gene = representative_gene
        .map(str::trim)
        .filter(|gene| !gene.is_empty())
        .unwrap_or(PRIMARY_VALIDATION_GENE);
    let name = auto_synth_tool_name(hypothesis_statement, capability_need)?;
    let description = format!(
        "Auto-synthesized tool for hypothesis {hypothesis_statement}. Capability need {capability_need}"
    );
    let script_path = synth_script_path(store.root_path(), &name);
    let fixture_path = auto_synth_fixture_path(store.root_path(), &name);
    let alternate_fixture_path = auto_synth_alternate_fixture_path(store.root_path(), &name);
    if let Some(parent) = script_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut prompt = base_prompt.clone();
    let mut last_rejection = "auto-synth did not produce a validated candidate".to_string();
    for _attempt in 1..=MAX_AUTO_SYNTH_ATTEMPTS {
        let raw_candidate = run_project_synthesizer(store.root_path(), synthesizer, &prompt)?;
        let candidate = match parse_auto_synth_candidate(&raw_candidate) {
            Ok(candidate) => candidate,
            Err(error) => {
                last_rejection = error.message();
                prompt =
                    build_auto_synth_repair_prompt(&base_prompt, &last_rejection, &raw_candidate);
                continue;
            }
        };

        fs::write(&script_path, candidate.script.as_bytes())?;
        fs::write(&fixture_path, candidate.fixture.as_bytes())?;
        fs::write(
            &alternate_fixture_path,
            candidate.alternate_fixture.as_bytes(),
        )?;
        let canonical_script_path = fs::canonicalize(&script_path)?;
        let canonical_fixture_path = fs::canonicalize(&fixture_path)?;
        let canonical_alternate_fixture_path = fs::canonicalize(&alternate_fixture_path)?;

        match validate_auto_synth_candidate(
            &canonical_script_path,
            &canonical_fixture_path,
            &canonical_alternate_fixture_path,
            &candidate,
            runtime_gene,
        )? {
            Ok(()) => {
                let spec_yaml =
                    synthesized_agent_tool_yaml(&name, &description, &canonical_script_path);
                let spec = ToolSpec::from_simple_yaml(&spec_yaml)?;
                let registration = store.register_tool(spec)?;
                return Ok(auto_synth_registered_result(
                    registration.tool_ref,
                    source_trace.as_deref(),
                ));
            }
            Err(reason) => {
                cleanup_auto_synth_candidate(
                    &canonical_script_path,
                    &[&canonical_fixture_path, &canonical_alternate_fixture_path],
                );
                last_rejection = reason;
                prompt = build_auto_synth_repair_prompt(
                    &base_prompt,
                    &last_rejection,
                    &candidate.script,
                );
            }
        }
    }

    let reason = if research_gap_on_failure {
        format!(
            "{}；合成候选未能通过真实数据运行时门，不编造结果，可能是真实研究空白。最后失败：{}",
            no_viable_public_source_reason(),
            last_rejection
        )
    } else {
        last_rejection
    };
    Ok(auto_synth_rejected_result(
        reason,
        source_trace.as_deref(),
        research_gap_on_failure,
    ))
}

fn auto_synth_registered_result(
    tool_ref: String,
    source_trace: Option<&str>,
) -> AutoSynthToolResult {
    match source_trace
        .map(str::trim)
        .filter(|trace| !trace.is_empty())
    {
        Some(trace) => AutoSynthToolResult::RegisteredWithSource {
            tool_ref,
            source_trace: trace.to_string(),
        },
        None => AutoSynthToolResult::Registered(tool_ref),
    }
}

fn auto_synth_rejected_result(
    reason: String,
    source_trace: Option<&str>,
    research_gap: bool,
) -> AutoSynthToolResult {
    match source_trace
        .map(str::trim)
        .filter(|trace| !trace.is_empty())
    {
        Some(trace) => AutoSynthToolResult::RejectedWithSource {
            reason,
            source_trace: trace.to_string(),
            research_gap,
        },
        None => AutoSynthToolResult::Rejected(reason),
    }
}

fn no_viable_public_source_reason() -> String {
    no_viable_public_source_reason_for("")
}

fn no_viable_public_source_reason_for(data_requirements: &str) -> String {
    let trimmed = data_requirements.trim();
    if trimmed.is_empty() {
        "未找到可访问公开数据源能直接提供回答该假设所需数据，可能是真实研究空白".to_string()
    } else {
        format!(
            "未找到可访问公开数据源能直接提供回答该假设所需数据（{trimmed}），可能是真实研究空白"
        )
    }
}

fn validate_auto_synth_candidate(
    script_path: &Path,
    fixture_path: &Path,
    alternate_fixture_path: &Path,
    candidate: &AutoSynthCandidate,
    runtime_gene: &str,
) -> Result<Result<(), String>, CliError> {
    let fixture_validation = validate_candidate_script_with_inputs(
        script_path,
        fixture_path,
        SynthValidationInputs {
            gene: PRIMARY_VALIDATION_GENE,
        },
    )?;
    if !auto_synth_validation_passed(&fixture_validation, &candidate.expect) {
        return Ok(Err(auto_synth_rejection_reason(
            "fixture smoke",
            &fixture_validation,
            &candidate.expect,
        )));
    }
    let alternate_validation = validate_candidate_script_with_inputs(
        script_path,
        alternate_fixture_path,
        SynthValidationInputs {
            gene: ALTERNATE_VALIDATION_GENE,
        },
    )?;
    if !auto_synth_validation_passed(&alternate_validation, &candidate.expect) {
        return Ok(Err(auto_synth_rejection_reason(
            "alternate fixture smoke",
            &alternate_validation,
            &candidate.expect,
        )));
    }
    let fixture_paths = &[fixture_path, alternate_fixture_path];
    if let Err(reason) =
        validate_input_sensitivity(&fixture_validation, &alternate_validation, fixture_paths)
    {
        return Ok(Err(reason));
    }
    let runtime_validation = validate_runtime_candidate_script(script_path, runtime_gene)?;
    if !auto_synth_validation_passed(&runtime_validation, &candidate.expect) {
        return Ok(Err(auto_synth_rejection_reason(
            "runtime gate",
            &runtime_validation,
            &candidate.expect,
        )));
    }
    if let Err(reason) = validate_runtime_gate_behavior(
        &runtime_validation,
        &fixture_validation,
        &alternate_validation,
        fixture_paths,
    ) {
        return Ok(Err(reason));
    }
    Ok(Ok(()))
}

fn require_option<T>(value: Option<T>, flag: &str) -> Result<T, CliError> {
    value.ok_or_else(|| CliError::InvalidArgument(format!("synth requires {flag}")))
}

fn validate_tool_name(name: &str) -> Result<(), CliError> {
    if name.trim().is_empty() {
        return Err(CliError::InvalidArgument(
            "--name must not be empty".to_string(),
        ));
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(CliError::InvalidArgument(
            "--name may only contain ASCII letters, numbers, underscore, dash, and dot".to_string(),
        ));
    }
    Ok(())
}

fn build_synth_prompt(description: &str) -> String {
    format!(
        concat!(
            "Write a self-contained Python 3 script using only the Python standard library.\n",
            "The script must read the input file path from the SYNTH_INPUT environment variable.\n",
            "The script must write its result to stdout.\n",
            "The script must compute its output from the real input data it reads.\n",
            "禁止硬编码、编造、default、sample、demo、placeholder 或 illustrative 数值/结论。\n",
            "If the required real data is unavailable, exit non-zero with a clear error instead of using fallback values.\n",
            "Task description:\n",
            "{}\n\n",
            "Return only raw Python code. Do not include markdown fences, explanations, or comments outside the code."
        ),
        description
    )
}

pub(crate) fn discover_cbioportal_grounding(hypothesis_statement: &str) -> Option<String> {
    discover_cbioportal_grounding_with_fetcher(hypothesis_statement, CBIOPORTAL_API_BASE, |url| {
        fetch_cbioportal_json_with_python(url, CBIOPORTAL_DISCOVERY_TIMEOUT)
    })
}

fn build_source_discovery_prompt(hypothesis_statement: &str, capability_need: &str) -> String {
    let allowlist = PUBLIC_SOURCE_ALLOWLIST
        .iter()
        .map(|host| format!("https://{host}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        concat!(
            "AgentFlow has a capability/data gap and needs autonomous public-source discovery before tool synthesis.\n",
            "Research hypothesis:\n{}\n\n",
            "Capability gap:\n{}\n\n",
            "First identify 回答该假设需要什么数据, for example: ICB/immunotherapy-treated cohort + response labels + gene expression/biomarker measurements.\n",
            "Use that required-data statement as the screening standard for every candidate.\n",
            "Propose candidate public scientific data sources that may contain real data for the hypothesis entity and endpoint need.\n",
            "Return ONLY a JSON array. Each object must contain string fields exactly: \"name\", \"base_url\", \"probe_url\", \"access_note\", \"required_data\", \"has_required_data\", \"required_data_reason\".\n",
            "\"required_data\" repeats the data needed to directly answer the hypothesis.\n",
            "\"has_required_data\" must be \"yes\" only when the source can directly answer the hypothesis with the required data; otherwise use \"no\".\n",
            "\"required_data_reason\" must explain the yes/no decision. If a source has related gene expression or survival data but lacks the endpoint needed by the question, mark \"has_required_data\":\"no\" and say related-but-insufficient.\n",
            "\"probe_url\" must be a read-only GET discovery/query endpoint that can verify data availability for the hypothesis entity.\n",
            "Allowed domains only: {}.\n",
            "Use only http(s) URLs. Do not propose localhost, private IP, file://, credentialed URLs, write endpoints, or non-public sources.\n",
            "If no suitable public source exists, return []."
        ),
        hypothesis_statement, capability_need, allowlist
    )
}

fn discover_public_sources_with_fetcher<F>(
    project_root: &Path,
    synthesizer: &str,
    hypothesis_statement: &str,
    capability_need: &str,
    fetch_probe: F,
) -> Result<SourceDiscoveryReport, CliError>
where
    F: FnMut(&str, Duration) -> Option<String>,
{
    let prompt = build_source_discovery_prompt(hypothesis_statement, capability_need);
    let raw = run_project_synthesizer(project_root, synthesizer, &prompt)?;
    let candidate_json = strip_markdown_fence(&raw);
    Ok(discover_sources_from_candidate_json(
        hypothesis_statement,
        capability_need,
        &candidate_json,
        fetch_probe,
    ))
}

fn discover_sources_from_candidate_json<F>(
    hypothesis_statement: &str,
    capability_need: &str,
    raw_candidates: &str,
    mut fetch_probe: F,
) -> SourceDiscoveryReport
where
    F: FnMut(&str, Duration) -> Option<String>,
{
    let proposal_was_json = looks_like_json_array(raw_candidates);
    let candidates = parse_public_source_candidates(raw_candidates);
    let relevance_terms = source_relevance_terms(hypothesis_statement, capability_need);
    let data_requirements =
        source_data_requirements(hypothesis_statement, capability_need, &candidates);
    let mut viable = Vec::new();
    let mut trace_lines = vec![
        "SOURCE DISCOVERY TRACE".to_string(),
        "QUESTION DATA REQUIREMENTS".to_string(),
        format!("required_data: {data_requirements}"),
        format!("candidate proposals parsed: {}", candidates.len()),
        format!("allowlist: {}", PUBLIC_SOURCE_ALLOWLIST.join(", ")),
    ];

    let mut probed_source_candidates = 0usize;
    for candidate in &candidates {
        let label = if candidate.name.trim().is_empty() {
            "<unnamed>"
        } else {
            candidate.name.trim()
        };
        let candidate_data_context = candidate_required_data_context(candidate);
        let base_host = match source_probe_safety(&candidate.base_url) {
            Ok(host) => host,
            Err(reason) => {
                trace_lines.push(format!(
                    "- {label}: skipped base_url {}; {reason}; {candidate_data_context}",
                    candidate.base_url,
                ));
                continue;
            }
        };
        let probe_host = match source_probe_safety(&candidate.probe_url) {
            Ok(host) => host,
            Err(reason) => {
                trace_lines.push(format!(
                    "- {label}: skipped probe_url {}; {reason}; {candidate_data_context}",
                    candidate.probe_url,
                ));
                continue;
            }
        };

        if probed_source_candidates >= MAX_PROBED_SOURCE_CANDIDATES {
            trace_lines.push(format!(
                "- {label}: skipped (probe budget {MAX_PROBED_SOURCE_CANDIDATES} reached)"
            ));
            continue;
        }
        probed_source_candidates += 1;

        let Some(body) = fetch_probe(&candidate.probe_url, SOURCE_DISCOVERY_TIMEOUT) else {
            trace_lines.push(format!(
                "- {label}: probed {probe_host}; failed or timed out; {candidate_data_context}"
            ));
            continue;
        };
        let trimmed = body.trim();
        if trimmed.is_empty() {
            trace_lines.push(format!(
                "- {label}: probed {probe_host}; empty response; {candidate_data_context}"
            ));
            continue;
        }

        match plausible_probe_summary(trimmed, &relevance_terms) {
            Some(summary) => {
                let required_data = assess_candidate_required_data(
                    candidate,
                    trimmed,
                    &data_requirements,
                    hypothesis_statement,
                    capability_need,
                );
                if !required_data.has_required_data {
                    let proxy_note = proxy_analysis_note(candidate, trimmed, &data_requirements);
                    trace_lines.push(format!(
                        "- {label}: related-but-insufficient; base_host={base_host}; probe_host={probe_host}; {summary}; {candidate_data_context}; has_required_data=no; reason={}; {}",
                        required_data.reason,
                        proxy_note
                    ));
                    continue;
                }
                trace_lines.push(format!(
                    "- {label}: viable; base_host={base_host}; probe_host={probe_host}; {summary}; {candidate_data_context}; has_required_data=yes; reason={}",
                    required_data.reason
                ));
                viable.push(ViablePublicSource {
                    candidate: candidate.clone(),
                    probe_summary: summary,
                });
            }
            None => {
                trace_lines.push(format!(
                    "- {label}: probed {probe_host}; non-empty but did not mention hypothesis terms; {candidate_data_context}; snippet={}",
                    snippet(trimmed),
                ));
            }
        }
    }

    SourceDiscoveryReport {
        candidates,
        viable,
        data_requirements,
        trace: trace_lines.join("\n"),
        proposal_was_json,
    }
}

fn parse_public_source_candidates(raw: &str) -> Vec<PublicSourceCandidate> {
    parse_json_string_objects(raw)
        .into_iter()
        .filter_map(|object| {
            let name = json_field(&object, "name")?.trim().to_string();
            let base_url = json_field(&object, "base_url")?.trim().to_string();
            let probe_url = json_field(&object, "probe_url")?.trim().to_string();
            let access_note = json_field(&object, "access_note")
                .unwrap_or("")
                .trim()
                .to_string();
            let required_data = json_field(&object, "required_data")
                .unwrap_or("")
                .trim()
                .to_string();
            let has_required_data = json_field(&object, "has_required_data")
                .unwrap_or("")
                .trim()
                .to_string();
            let required_data_reason = json_field(&object, "required_data_reason")
                .or_else(|| json_field(&object, "reason"))
                .unwrap_or("")
                .trim()
                .to_string();
            Some(PublicSourceCandidate {
                name,
                base_url,
                probe_url,
                access_note,
                required_data,
                has_required_data,
                required_data_reason,
            })
        })
        .collect()
}

fn looks_like_json_array(raw: &str) -> bool {
    raw.trim_start().starts_with('[')
}

fn source_probe_safety(url: &str) -> Result<String, String> {
    let host = http_url_host(url)?;
    if host == "localhost" || host.ends_with(".localhost") {
        return Err(format!("localhost host {host} is not allowed"));
    }
    if let Ok(ip) = IpAddr::from_str(&host) {
        if is_private_or_local_ip(ip) {
            return Err(format!("private or local IP {host} is not allowed"));
        }
        return Err(format!("host {host} is not allowlisted"));
    }
    if !PUBLIC_SOURCE_ALLOWLIST.contains(&host.as_str()) {
        return Err(format!("host {host} is not allowlisted"));
    }
    Ok(host)
}

fn http_url_host(url: &str) -> Result<String, String> {
    let trimmed = url.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return Err("URL must include an http(s) scheme".to_string());
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return Err("probe_url must use http(s)".to_string());
    }
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    if authority.is_empty() {
        return Err("URL host must not be empty".to_string());
    }
    if authority.contains('@') {
        return Err("credentialed URLs are not allowed".to_string());
    }
    let host = if let Some(after_bracket) = authority.strip_prefix('[') {
        let Some((host, _rest)) = after_bracket.split_once(']') else {
            return Err("invalid IPv6 URL host".to_string());
        };
        host
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        Err("URL host must not be empty".to_string())
    } else {
        Ok(host)
    }
}

fn is_private_or_local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => {
            let octets = ipv4.octets();
            ipv4.is_private()
                || ipv4.is_loopback()
                || ipv4.is_link_local()
                || ipv4.is_unspecified()
                || octets[0] == 0
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        }
        IpAddr::V6(ipv6) => {
            let first = ipv6.segments()[0];
            ipv6.is_loopback()
                || ipv6.is_unspecified()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
        }
    }
}

fn source_relevance_terms(hypothesis_statement: &str, capability_need: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for token in hypothesis_statement
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-')
        .chain(capability_need.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-'))
        .map(|token| token.trim_matches('-'))
        .filter(|token| token.len() >= 4)
    {
        let keep = is_source_gene_symbol_candidate(token)
            || matches!(
                token.to_ascii_lowercase().as_str(),
                "immunotherapy"
                    | "immune"
                    | "response"
                    | "checkpoint"
                    | "melanoma"
                    | "hepatocellular"
                    | "carcinoma"
                    | "survival"
                    | "expression"
            );
        if keep {
            let normalized = token.to_ascii_lowercase();
            if !terms.iter().any(|existing| existing == &normalized) {
                terms.push(normalized);
            }
        }
    }
    terms
}

fn source_data_requirements(
    hypothesis_statement: &str,
    capability_need: &str,
    candidates: &[PublicSourceCandidate],
) -> String {
    candidates
        .iter()
        .find_map(|candidate| {
            let required_data = candidate.required_data.trim();
            (!required_data.is_empty()).then(|| required_data.to_string())
        })
        .unwrap_or_else(|| question_data_requirements(hypothesis_statement, capability_need))
}

fn question_data_requirements(hypothesis_statement: &str, capability_need: &str) -> String {
    let lower = format!("{hypothesis_statement} {capability_need}").to_ascii_lowercase();
    if needs_immunotherapy_response(&lower) {
        return "ICB/immunotherapy-treated cohort + response labels + gene expression/biomarker measurements".to_string();
    }
    if contains_any(
        &lower,
        &["survival", "overall survival", "progression-free"],
    ) {
        return "cohort with gene expression/biomarker measurements + survival or outcome labels"
            .to_string();
    }
    if contains_any(&lower, &["expression", "rna", "mrna", "transcript"]) {
        return "cohort with gene expression measurements for the hypothesis entity".to_string();
    }
    "public cohort data containing the hypothesis entity and the endpoint measurements needed to answer the question".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequiredDataAssessment {
    has_required_data: bool,
    reason: String,
}

fn assess_candidate_required_data(
    candidate: &PublicSourceCandidate,
    probe_body: &str,
    data_requirements: &str,
    hypothesis_statement: &str,
    capability_need: &str,
) -> RequiredDataAssessment {
    let proposed = parse_required_data_assessment(&candidate.has_required_data);
    if proposed == Some(false) {
        return RequiredDataAssessment {
            has_required_data: false,
            reason: candidate_required_data_reason(candidate)
                .unwrap_or_else(|| "candidate was marked related-but-insufficient".to_string()),
        };
    }

    if let Some(missing) = missing_required_data_reason(
        probe_body,
        data_requirements,
        hypothesis_statement,
        capability_need,
    ) {
        return RequiredDataAssessment {
            has_required_data: false,
            reason: missing,
        };
    }

    RequiredDataAssessment {
        has_required_data: true,
        reason: candidate_required_data_reason(candidate).unwrap_or_else(|| {
            if proposed == Some(true) {
                "candidate marked has_required_data=yes and probe verified the required data"
                    .to_string()
            } else {
                "probe verified the required data for this question".to_string()
            }
        }),
    }
}

fn missing_required_data_reason(
    probe_body: &str,
    data_requirements: &str,
    hypothesis_statement: &str,
    capability_need: &str,
) -> Option<String> {
    let lower_question = format!("{hypothesis_statement} {capability_need} {data_requirements}")
        .to_ascii_lowercase();
    let lower_probe = probe_body.to_ascii_lowercase();

    if needs_immunotherapy_response(&lower_question) {
        let mut missing = Vec::new();
        if !contains_any(
            &lower_probe,
            &[
                "icb",
                "immunotherapy",
                "immune checkpoint",
                "checkpoint blockade",
                "anti-pd",
                "anti pd",
                "pd-1",
                "pd1",
                "pd-l1",
                "pdl1",
                "ctla-4",
                "ctla4",
            ],
        ) {
            missing.push("ICB/immunotherapy-treated cohort");
        }
        if !contains_any(
            &lower_probe,
            &[
                "response",
                "responder",
                "non-responder",
                "nonresponder",
                "recist",
                "objective response",
                "orr",
                "clinical benefit",
            ],
        ) {
            missing.push("response labels");
        }
        if contains_any(
            &lower_question,
            &["expression", "gene expression", "biomarker", "rna", "mrna"],
        ) && !contains_any(
            &lower_probe,
            &[
                "expression",
                "gene expression",
                "rna",
                "mrna",
                "transcript",
                "biomarker",
                "mutation",
            ],
        ) {
            missing.push("gene expression/biomarker measurements");
        }
        if !missing.is_empty() {
            return Some(format!(
                "probe did not verify required data: {}",
                missing.join(" + ")
            ));
        }
    }

    None
}

fn needs_immunotherapy_response(lower_text: &str) -> bool {
    contains_any(
        lower_text,
        &[
            "icb",
            "immunotherapy",
            "immune checkpoint",
            "checkpoint blockade",
            "anti-pd",
            "pd-1",
            "pd-l1",
            "ctla-4",
        ],
    ) && contains_any(
        lower_text,
        &[
            "response",
            "responder",
            "non-responder",
            "nonresponder",
            "clinical benefit",
            "benefit",
            "orr",
            "recist",
        ],
    )
}

fn candidate_required_data_context(candidate: &PublicSourceCandidate) -> String {
    format!(
        "required_data={}; proposed_has_required_data={}; required_data_reason={}",
        nonempty_or(&candidate.required_data, "unspecified"),
        nonempty_or(&candidate.has_required_data, "unspecified"),
        nonempty_or(&candidate.required_data_reason, "unspecified")
    )
}

fn candidate_required_data_reason(candidate: &PublicSourceCandidate) -> Option<String> {
    let reason = candidate.required_data_reason.trim();
    (!reason.is_empty()).then(|| reason.to_string())
}

fn parse_required_data_assessment(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "y" | "has_required_data" | "是" => Some(true),
        "no" | "false" | "n" | "related-but-insufficient" | "insufficient" | "否" => Some(false),
        _ => None,
    }
}

fn proxy_analysis_note(
    candidate: &PublicSourceCandidate,
    probe_body: &str,
    data_requirements: &str,
) -> String {
    let lower = format!(
        "{} {} {} {}",
        candidate.name, candidate.access_note, candidate.required_data_reason, probe_body
    )
    .to_ascii_lowercase();
    if contains_any(
        &lower,
        &[
            "cbioportal",
            "expression",
            "gene expression",
            "survival",
            "overall survival",
            "rna",
            "mrna",
        ],
    ) {
        format!(
            "存在代理分析但不直接回答本问题：相关表达/生存数据不能替代 {}",
            data_requirements
        )
    } else {
        "proxy_analysis_note=none".to_string()
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn nonempty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback
    } else {
        trimmed
    }
}

fn is_source_gene_symbol_candidate(token: &str) -> bool {
    let token = token.trim();
    if !(2..=20).contains(&token.len()) {
        return false;
    }
    if token
        .chars()
        .any(|ch| !ch.is_ascii_alphanumeric() && ch != '-')
    {
        return false;
    }
    if !token.bytes().any(|byte| byte.is_ascii_alphabetic()) {
        return false;
    }
    let uppercase = token.to_ascii_uppercase();
    if token != uppercase {
        return false;
    }
    !matches!(
        uppercase.as_str(),
        "AUTO" | "SYNTH" | "TCGA" | "RNA" | "DNA" | "API" | "REST" | "LLM" | "ICB"
    )
}

fn plausible_probe_summary(body: &str, relevance_terms: &[String]) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let matched = relevance_terms
        .iter()
        .filter(|term| lower.contains(term.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !relevance_terms.is_empty() && matched.is_empty() {
        return None;
    }
    let matched_text = if matched.is_empty() {
        "matched_terms=none-required".to_string()
    } else {
        format!("matched_terms={}", matched.join(","))
    };
    Some(format!(
        "non_empty_response_bytes={}; {}; snippet={}",
        body.len(),
        matched_text,
        snippet(body)
    ))
}

fn discover_cbioportal_grounding_with_fetcher<F>(
    hypothesis_statement: &str,
    api_base: &str,
    mut fetch_json: F,
) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    let cancer_terms = cancer_terms_for_hypothesis(hypothesis_statement);
    if cancer_terms.is_empty() {
        return None;
    }

    let api_base = api_base.trim_end_matches('/');
    let studies = parse_json_string_objects(&fetch_json(&format!("{api_base}/studies"))?);
    let study_id = choose_cbioportal_study(&studies, &cancer_terms)?;
    let encoded_study_id = url_path_component(&study_id);
    let profiles = parse_json_string_objects(&fetch_json(&format!(
        "{api_base}/studies/{encoded_study_id}/molecular-profiles"
    ))?);
    let mrna_profile_id = choose_cbioportal_mrna_profile(&profiles)?;
    let sample_lists = parse_json_string_objects(&fetch_json(&format!(
        "{api_base}/studies/{encoded_study_id}/sample-lists"
    ))?);
    let sample_list_id = choose_cbioportal_sample_list(&sample_lists, &study_id)?;

    Some(format!(
        "Discovered real cBioPortal identifiers: studyId={study_id}, mrnaMolecularProfileId={mrna_profile_id}, sampleListId={sample_list_id}, api_base={api_base}. Use these EXACT identifiers; do not guess."
    ))
}

fn cancer_terms_for_hypothesis(statement: &str) -> Vec<&'static str> {
    const CANCER_TERM_GROUPS: &[(&[&str], &[&str])] = &[
        (
            &["liver", "hepatocellular", "hcc", "lihc"],
            &["lihc", "hepatocellular", "liver", "hcc"],
        ),
        (&["breast", "brca"], &["brca", "breast"]),
        (
            &["lung", "luad", "lusc", "nsclc"],
            &["luad", "lusc", "lung", "nsclc"],
        ),
        (
            &["colon", "colorectal", "coad", "read"],
            &["coad", "read", "colorectal", "colon"],
        ),
        (&["prostate", "prad"], &["prad", "prostate"]),
        (&["ovarian", "ovary", "ov"], &["ov", "ovarian"]),
        (&["melanoma", "skcm"], &["skcm", "melanoma"]),
        (&["pancreatic", "pancreas", "paad"], &["paad", "pancreatic"]),
        (&["glioblastoma", "gbm"], &["gbm", "glioblastoma"]),
        (
            &["kidney", "renal", "kirc", "kirp"],
            &["kirc", "kirp", "kidney", "renal"],
        ),
        (
            &["gastric", "stomach", "stad"],
            &["stad", "gastric", "stomach"],
        ),
        (&["bladder", "blca"], &["blca", "bladder"]),
        (
            &["endometrial", "uterine", "ucec"],
            &["ucec", "endometrial", "uterine"],
        ),
        (&["head and neck", "hnsc"], &["hnsc", "head and neck"]),
        (&["thyroid", "thca"], &["thca", "thyroid"]),
        (&["leukemia", "aml", "laml"], &["laml", "aml", "leukemia"]),
    ];

    let lower = statement.to_ascii_lowercase();
    let mut terms = Vec::new();
    for (needles, group_terms) in CANCER_TERM_GROUPS {
        if needles
            .iter()
            .any(|needle| contains_domain_term(&lower, needle))
        {
            for term in *group_terms {
                if !terms.contains(term) {
                    terms.push(*term);
                }
            }
        }
    }
    terms
}

fn choose_cbioportal_study(studies: &[JsonObject], cancer_terms: &[&str]) -> Option<String> {
    studies
        .iter()
        .filter_map(|study| {
            let study_id = json_field(study, "studyId")?;
            let score = score_cbioportal_study(study, cancer_terms)?;
            Some((study_id.to_string(), score))
        })
        .max_by_key(|(_, score)| *score)
        .map(|(study_id, _)| study_id)
}

fn score_cbioportal_study(study: &JsonObject, cancer_terms: &[&str]) -> Option<i32> {
    let study_id = json_field(study, "studyId")?.to_ascii_lowercase();
    let cancer_type = json_field(study, "cancerTypeId")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = json_field(study, "name")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let description = json_field(study, "description")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let haystack = format!("{study_id} {cancer_type} {name} {description}");
    let mut score = 0;
    let mut matched_cancer = false;

    for term in cancer_terms {
        if contains_domain_term(&study_id, term) || contains_domain_term(&cancer_type, term) {
            score += 45;
            matched_cancer = true;
        } else if contains_domain_term(&haystack, term) {
            score += 20;
            matched_cancer = true;
        }
    }
    if !matched_cancer {
        return None;
    }
    if study_id.contains("pan_can_atlas") || name.contains("pancancer atlas") {
        score += 80;
    }
    if study_id.contains("tcga") {
        score += 25;
    }
    if study_id.contains("2018") {
        score += 5;
    }
    if study_id.contains("cell_line") || study_id.contains("ccle") {
        score -= 30;
    }
    Some(score)
}

fn choose_cbioportal_mrna_profile(profiles: &[JsonObject]) -> Option<String> {
    profiles
        .iter()
        .filter_map(|profile| {
            let profile_id = json_field(profile, "molecularProfileId")?;
            let score = score_cbioportal_mrna_profile(profile)?;
            Some((profile_id.to_string(), score))
        })
        .max_by_key(|(_, score)| *score)
        .map(|(profile_id, _)| profile_id)
}

fn score_cbioportal_mrna_profile(profile: &JsonObject) -> Option<i32> {
    let profile_id = json_field(profile, "molecularProfileId")?.to_ascii_lowercase();
    let alteration_type = json_field(profile, "molecularAlterationType")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let datatype = json_field(profile, "datatype")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = json_field(profile, "name")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let description = json_field(profile, "description")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let haystack = format!("{profile_id} {alteration_type} {datatype} {name} {description}");
    let mut score = 0;

    if alteration_type == "mrna_expression" {
        score += 100;
    }
    if haystack.contains("mrna") {
        score += 35;
    }
    if haystack.contains("rna_seq") || haystack.contains("rna seq") {
        score += 30;
    }
    if haystack.contains("expression") {
        score += 20;
    }
    if datatype == "continuous" {
        score += 20;
    }
    if profile_id.contains("rna_seq_v2_mrna") {
        score += 70;
    }
    if profile_id.ends_with("_mrna") {
        score += 20;
    }
    if haystack.contains("zscore") || haystack.contains("z-score") {
        score -= 25;
    }
    if score <= 0 {
        return None;
    }
    Some(score)
}

fn choose_cbioportal_sample_list(sample_lists: &[JsonObject], study_id: &str) -> Option<String> {
    sample_lists
        .iter()
        .filter_map(|sample_list| {
            let sample_list_id = json_field(sample_list, "sampleListId")?;
            let score = score_cbioportal_sample_list(sample_list, study_id)?;
            Some((sample_list_id.to_string(), score))
        })
        .max_by_key(|(_, score)| *score)
        .map(|(sample_list_id, _)| sample_list_id)
}

fn score_cbioportal_sample_list(sample_list: &JsonObject, study_id: &str) -> Option<i32> {
    let sample_list_id = json_field(sample_list, "sampleListId")?.to_ascii_lowercase();
    let category = json_field(sample_list, "category")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = json_field(sample_list, "name")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let description = json_field(sample_list, "description")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let haystack = format!("{sample_list_id} {category} {name} {description}");
    let mut score = 0;
    let all_id = format!("{}_all", study_id.to_ascii_lowercase());

    if sample_list_id == all_id {
        score += 120;
    }
    if sample_list_id.ends_with("_all") {
        score += 80;
    }
    if category.contains("all_cases") || haystack.contains("all samples") {
        score += 45;
    }
    if sample_list_id.contains("sequenced") {
        score += 15;
    }
    if score <= 0 {
        return None;
    }
    Some(score)
}

fn fetch_cbioportal_json_with_python(url: &str, timeout: Duration) -> Option<String> {
    let mut command = Command::new("/usr/bin/env");
    command
        .arg("python3")
        .arg("-c")
        .arg(CBIOPORTAL_DISCOVERY_FETCH_PY)
        .arg(url)
        .arg(timeout.as_secs_f64().to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_child_process_group(&mut command);
    let mut child = command.spawn().ok()?;
    let started = SystemTime::now();

    loop {
        if child.try_wait().ok()?.is_some() {
            let output = child.wait_with_output().ok()?;
            if !output.status.success() {
                return None;
            }
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            return (!stdout.trim().is_empty()).then_some(stdout);
        }

        if started.elapsed().unwrap_or_default() >= timeout {
            kill_child_process_group(&mut child);
            let _ = child.wait();
            return None;
        }

        thread::sleep(Duration::from_millis(20));
    }
}

fn fetch_public_source_probe_with_python(url: &str, timeout: Duration) -> Option<String> {
    let mut command = Command::new("/usr/bin/env");
    command
        .arg("python3")
        .arg("-c")
        .arg(SOURCE_PROBE_FETCH_PY)
        .arg(url)
        .arg(timeout.as_secs_f64().to_string())
        .arg(MAX_SOURCE_PROBE_BYTES.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_child_process_group(&mut command);
    let mut child = command.spawn().ok()?;
    let started = SystemTime::now();

    loop {
        if child.try_wait().ok()?.is_some() {
            let output = child.wait_with_output().ok()?;
            if !output.status.success() {
                return None;
            }
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            return (!stdout.trim().is_empty()).then_some(stdout);
        }

        if started.elapsed().unwrap_or_default() >= timeout {
            kill_child_process_group(&mut child);
            let _ = child.wait();
            return None;
        }

        thread::sleep(Duration::from_millis(20));
    }
}

fn parse_json_string_objects(json: &str) -> Vec<JsonObject> {
    let mut objects = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut depth = 0usize;
    let mut object_start = 0usize;

    for (index, ch) in json.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    object_start = index;
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let object = parse_json_object_string_fields(&json[object_start..=index]);
                    if !object.is_empty() {
                        objects.push(object);
                    }
                }
            }
            _ => {}
        }
    }

    objects
}

fn parse_json_object_string_fields(object: &str) -> JsonObject {
    let mut fields = JsonObject::new();
    let mut cursor = 0usize;

    while let Some(relative_start) = object[cursor..].find('"') {
        let key_start = cursor + relative_start;
        let Some((key, after_key)) = parse_json_string_at(object, key_start) else {
            break;
        };
        let colon = skip_json_whitespace(object, after_key);
        if !object[colon..].starts_with(':') {
            cursor = after_key;
            continue;
        }
        let value_start = skip_json_whitespace(object, colon + 1);
        if !object[value_start..].starts_with('"') {
            cursor = value_start;
            continue;
        }
        let Some((value, after_value)) = parse_json_string_at(object, value_start) else {
            break;
        };
        fields.insert(key, value);
        cursor = after_value;
    }

    fields
}

fn parse_json_string_at(value: &str, start: usize) -> Option<(String, usize)> {
    if !value[start..].starts_with('"') {
        return None;
    }
    let mut output = String::new();
    let mut chars = value[start + 1..].char_indices().peekable();

    while let Some((offset, ch)) = chars.next() {
        let absolute_index = start + 1 + offset;
        match ch {
            '"' => return Some((output, absolute_index + ch.len_utf8())),
            '\\' => {
                let (_, escaped) = chars.next()?;
                match escaped {
                    '"' => output.push('"'),
                    '\\' => output.push('\\'),
                    '/' => output.push('/'),
                    'b' => output.push('\u{0008}'),
                    'f' => output.push('\u{000c}'),
                    'n' => output.push('\n'),
                    'r' => output.push('\r'),
                    't' => output.push('\t'),
                    'u' => {
                        let mut hex = String::new();
                        for _ in 0..4 {
                            let (_, digit) = chars.next()?;
                            hex.push(digit);
                        }
                        let codepoint = u32::from_str_radix(&hex, 16).ok()?;
                        output.push(char::from_u32(codepoint)?);
                    }
                    other => output.push(other),
                }
            }
            other => output.push(other),
        }
    }

    None
}

fn skip_json_whitespace(value: &str, start: usize) -> usize {
    value[start..]
        .char_indices()
        .find_map(|(offset, ch)| (!ch.is_whitespace()).then_some(start + offset))
        .unwrap_or(value.len())
}

fn json_field<'a>(object: &'a JsonObject, key: &str) -> Option<&'a str> {
    object
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn contains_domain_term(haystack: &str, term: &str) -> bool {
    if term.chars().all(|ch| ch.is_ascii_alphanumeric()) && term.len() <= 4 {
        contains_ascii_word(haystack, term)
    } else {
        haystack.contains(term)
    }
}

fn contains_ascii_word(haystack: &str, needle: &str) -> bool {
    let mut search_start = 0usize;
    while let Some(relative_index) = haystack[search_start..].find(needle) {
        let start = search_start + relative_index;
        let end = start + needle.len();
        let before = haystack[..start].chars().next_back();
        let after = haystack[end..].chars().next();
        let before_boundary = before.is_none_or(|ch| !ch.is_ascii_alphanumeric());
        let after_boundary = after.is_none_or(|ch| !ch.is_ascii_alphanumeric());
        if before_boundary && after_boundary {
            return true;
        }
        search_start = end;
    }
    false
}

fn url_path_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn build_prompt_for_viable_source(
    hypothesis_statement: &str,
    capability_need: &str,
    viable: &ViablePublicSource,
    source_trace: &str,
    cbioportal_grounding: Option<&str>,
) -> String {
    if is_cbioportal_source(viable) {
        let mut grounding = String::new();
        if let Some(cbioportal_grounding) = cbioportal_grounding
            .map(str::trim)
            .filter(|grounding| !grounding.is_empty())
        {
            grounding.push_str(cbioportal_grounding);
            grounding.push('\n');
        }
        grounding.push_str("SOURCE DISCOVERY TRACE:\n");
        grounding.push_str(source_trace);
        build_auto_synth_prompt(hypothesis_statement, capability_need, Some(&grounding))
    } else {
        build_public_source_auto_synth_prompt(
            hypothesis_statement,
            capability_need,
            viable,
            source_trace,
        )
    }
}

fn is_cbioportal_source(viable: &ViablePublicSource) -> bool {
    source_probe_safety(&viable.candidate.base_url).as_deref() == Ok("www.cbioportal.org")
        || source_probe_safety(&viable.candidate.probe_url).as_deref() == Ok("www.cbioportal.org")
}

fn build_public_source_auto_synth_prompt(
    hypothesis_statement: &str,
    capability_need: &str,
    viable: &ViablePublicSource,
    source_trace: &str,
) -> String {
    format!(
        concat!(
            "You are writing an AgentFlow exploratory analysis tool grounded on a system-probed public scientific source.\n",
            "Use Python 3 standard library only. urllib.request is allowed only for read-only GET requests to the grounded source below.\n",
            "Do not access non-allowlisted domains, localhost, private IPs, file:// URLs, credentialed URLs, write endpoints, or arbitrary web searches.\n",
            "Grounded public source selected by the system:\n",
            "name: {}\n",
            "base_url: {}\n",
            "probe_url: {}\n",
            "access_note: {}\n",
            "probe_summary: {}\n\n",
            "SOURCE DISCOVERY TRACE:\n{}\n\n",
            "The generated tool spec will declare a required domain parameter named gene, so runtime receives AGENTFLOW_PARAM_GENE.\n",
            "The tool must support two modes with the same calculation logic:\n",
            "1. Runtime mode: when SYNTH_INPUT is unset, read domain parameters from AGENTFLOW_PARAM_<UPPER_NAME>, especially AGENTFLOW_PARAM_GENE.\n",
            "   Fetch real public data from the grounded source using read-only GET and compute an honest result. Write AGENTFLOW_OUTPUT_RESULT and print stdout.\n",
            "2. Validation mode: when SYNTH_INPUT is set, read that fixture file, run the same deterministic calculation logic offline, write AGENTFLOW_OUTPUT_RESULT, and print stdout.\n",
            "禁止硬编码或编造 HR、p-value、correlation、effect size、biomarker grade、sample count, response rate, or any other numeric/stance result.\n",
            "Do not use DEFAULT_PANEL, default, demo, sample, placeholder, illustrative, toy, or fallback conclusions.\n",
            "If the public source lacks usable real data, exit non-zero with a clear stderr error. If the public source lacks usable real data, exit non-zero instead of inventing data.\n",
            "During validation, SYNTH_INPUT will point to two meaningfully different fixtures; AGENTFLOW_PARAM_GENE will be TP53 for the first fixture and EGFR for the second fixture; normalized output must change when these inputs change.\n\n",
            "Research hypothesis:\n{}\n\n",
            "Capability gap:\n{}\n\n",
            "Return exactly four sections with these markers and no extra text:\n",
            "===SCRIPT===\n",
            "<raw Python code>\n",
            "===FIXTURE===\n",
            "<small fixture text for SYNTH_INPUT>\n",
            "===ALT_FIXTURE===\n",
            "<second fixture with materially different values/entities from FIXTURE>\n",
            "===EXPECT===\n",
            "<one substring that must appear in stdout and AGENTFLOW_OUTPUT_RESULT>\n"
        ),
        viable.candidate.name,
        viable.candidate.base_url,
        viable.candidate.probe_url,
        viable.candidate.access_note,
        viable.probe_summary,
        source_trace,
        hypothesis_statement,
        capability_need
    )
}

fn build_auto_synth_prompt(
    hypothesis_statement: &str,
    capability_need: &str,
    grounding: Option<&str>,
) -> String {
    let grounding_section = grounding
        .map(str::trim)
        .filter(|block| !block.is_empty())
        .map(|block| {
            format!(
                concat!(
                    "\nIMPORTANT LIVE API GROUNDING (queried immediately before synthesis):\n",
                    "{}\n",
                    "When using cBioPortal through the verified client, use the exact studyId, mRNA molecular profile id, sample list id, and api_base above as grounding context. ",
                    "Do not substitute remembered identifiers.\n\n"
                ),
                block
            )
        })
        .unwrap_or_default();
    format!(
        concat!(
            "You are writing an AgentFlow exploratory analysis tool. Use Python 3 standard library plus the provided verified cBioPortal client module.\n",
            "{}",
            "The generated tool spec will declare a required domain parameter named gene, so runtime receives AGENTFLOW_PARAM_GENE.\n",
            "Verified client extracted from examples/tools/tcga_survival_assoc.py:\n",
            "import agentflow_cbioportal as cbio\n",
            "resolve_study(cancer_keyword) -> str\n",
            "    Return a real cBioPortal study id for a cancer keyword; prefers TCGA PanCancer Atlas; raises if unavailable.\n",
            "fetch_expression(study_id, gene) -> dict\n",
            "    Return sample_id -> mRNA expression value using the resolved mRNA profile and molecular-data POST fetch.\n",
            "fetch_overall_survival(study_id) -> dict\n",
            "    Return patient_id -> (OS_MONTHS, os_event_bool) using clinical-data POST fetch.\n",
            "fetch_clinical_attribute(study_id, attr) -> dict\n",
            "    Return patient_id -> clinical attribute value using clinical-data POST fetch.\n",
            "Use the client in runtime mode: import agentflow_cbioportal, call cbio.resolve_study/cbio.fetch_expression/cbio.fetch_overall_survival/cbio.fetch_clinical_attribute as needed.\n",
            "do not write HTTP/API calls, urllib calls, requests calls, endpoint discovery, or cBioPortal client code yourself; only write analysis logic.\n",
            "For cBioPortal data the verified client uses https://www.cbioportal.org/api and raises instead of fabricating data.\n",
            "The tool must support two modes with the same calculation logic:\n",
            "1. Runtime mode: when SYNTH_INPUT is unset, read domain parameters from AGENTFLOW_PARAM_<UPPER_NAME>, especially AGENTFLOW_PARAM_GENE.\n",
            "   Fetch real cBioPortal data through agentflow_cbioportal, then compute association/grouping/statistics yourself.\n",
            "   Write the main Markdown/Text result to the path in AGENTFLOW_OUTPUT_RESULT and also print it to stdout.\n",
            "2. Validation mode: when SYNTH_INPUT is set, read that fixture file, run the same deterministic calculation logic offline, write AGENTFLOW_OUTPUT_RESULT, and print stdout.\n",
            "禁止硬编码或编造 HR、p-value、correlation、effect size、biomarker grade、sample count, or any other numeric/stance result.\n",
            "Do not use DEFAULT_PANEL, default, demo, sample, placeholder, illustrative, toy, or fallback conclusions.\n",
            "真实数据不可得时必须 loudly fail: print a clear error to stderr and 非零退出 (exit non-zero).\n",
            "Never silently succeed with default/illustrative fallback data.\n",
            "During validation, SYNTH_INPUT will point to two meaningfully different fixtures; AGENTFLOW_PARAM_GENE will be TP53 for the first fixture and EGFR for the second fixture; the normalized output must change when these inputs change.\n",
            "If neither SYNTH_INPUT fixture data nor runtime real public data is available, exit non-zero instead of inventing data.\n\n",
            "Minimal runtime shape:\n",
            "import os\n",
            "from pathlib import Path\n",
            "import agentflow_cbioportal as cbio\n",
            "gene = os.environ.get(\"AGENTFLOW_PARAM_GENE\")\n",
            "out = os.environ.get(\"AGENTFLOW_OUTPUT_RESULT\")\n",
            "if not gene or not out: raise SystemExit(\"AGENTFLOW_PARAM_GENE and AGENTFLOW_OUTPUT_RESULT are required\")\n",
            "study = cbio.resolve_study(\"hepatocellular carcinoma\")\n",
            "expr = cbio.fetch_expression(study, gene)\n",
            "surv = cbio.fetch_overall_survival(study)\n",
            "# analyze expr/surv honestly; if required data is absent, let the client exception fail loudly\n",
            "Path(out).write_text(\"...real computed result...\\n\", encoding=\"utf-8\")\n\n",
            "Research hypothesis:\n{}\n\n",
            "Capability gap:\n{}\n\n",
            "Return exactly four sections with these markers and no extra text:\n",
            "===SCRIPT===\n",
            "<raw Python code>\n",
            "===FIXTURE===\n",
            "<small fixture text for SYNTH_INPUT>\n",
            "===ALT_FIXTURE===\n",
            "<second fixture with materially different values/entities from FIXTURE>\n",
            "===EXPECT===\n",
            "<one substring that must appear in stdout and AGENTFLOW_OUTPUT_RESULT>\n"
        ),
        grounding_section,
        hypothesis_statement,
        capability_need
    )
}

fn build_auto_synth_repair_prompt(base_prompt: &str, error: &str, candidate_code: &str) -> String {
    format!(
        concat!(
            "{}\n\n",
            "你的工具运行失败,错误如下:\n{}\n\n",
            "这是代码:\n{}\n\n",
            "修正它,仍遵守 no-fabrication 与双模契约: ",
            "SYNTH_INPUT fixture validation must stay deterministic and input-sensitive; ",
            "runtime mode must read AGENTFLOW_PARAM_GENE, use the grounded public source instructions in the base prompt, write AGENTFLOW_OUTPUT_RESULT, ",
            "for cBioPortal specifically import/use agentflow_cbioportal instead of writing HTTP/API calls; for non-cBioPortal sources use only standard-library read-only GET to the grounded allowlisted source, ",
            "print non-empty output, and loudly exit non-zero instead of fabricating fallback/default data. ",
            "Return exactly the same four sections: ===SCRIPT===, ===FIXTURE===, ===ALT_FIXTURE===, ===EXPECT===."
        ),
        base_prompt,
        error,
        candidate_code
    )
}

fn parse_auto_synth_candidate(candidate: &str) -> Result<AutoSynthCandidate, CliError> {
    let script = strip_markdown_fence(&required_section(candidate, "SCRIPT")?);
    let fixture = strip_markdown_fence(&required_section(candidate, "FIXTURE")?);
    let alternate_fixture = strip_markdown_fence(&required_section(candidate, "ALT_FIXTURE")?);
    let expect = required_section(candidate, "EXPECT")?
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string();

    if script.trim().is_empty()
        || fixture.trim().is_empty()
        || alternate_fixture.trim().is_empty()
        || expect.trim().is_empty()
    {
        return Err(CliError::Core(
            "auto-synth candidate is missing script, fixture, alternate fixture, or expect section"
                .to_string(),
        ));
    }
    if normalize_fixture_for_comparison(&fixture)
        == normalize_fixture_for_comparison(&alternate_fixture)
    {
        return Err(CliError::Core(
            "auto-synth candidate fixtures are not meaningfully different".to_string(),
        ));
    }

    Ok(AutoSynthCandidate {
        script,
        fixture,
        alternate_fixture,
        expect,
    })
}

fn required_section(candidate: &str, name: &str) -> Result<String, CliError> {
    let marker = format!("==={name}===");
    let mut in_section = false;
    let mut lines = Vec::new();
    for line in candidate.lines() {
        let trimmed = line.trim();
        if trimmed == marker {
            in_section = true;
            continue;
        }
        if trimmed.starts_with("===") && trimmed.ends_with("===") && in_section {
            break;
        }
        if in_section {
            lines.push(line);
        }
    }
    if !in_section {
        return Err(CliError::Core(format!(
            "auto-synth candidate is missing {marker}"
        )));
    }
    Ok(lines.join("\n").trim().to_string())
}

pub(crate) fn run_project_synthesizer(
    project_root: &Path,
    command_line: &str,
    prompt: &str,
) -> Result<String, CliError> {
    let env = crate::llm_commands::load_project_llm_env(project_root)?;
    run_synthesizer_with_env(command_line, prompt, &env)
}

pub(crate) fn configured_or_default_synthesizer(
    project_root: &Path,
    explicit: Option<String>,
) -> Result<String, CliError> {
    if let Some(explicit) = explicit {
        return Ok(explicit);
    }
    Ok(crate::llm_commands::configured_synthesizer(project_root)?
        .unwrap_or_else(|| DEFAULT_SYNTHESIZER.to_string()))
}

fn run_synthesizer_with_env(
    command_line: &str,
    prompt: &str,
    env: &[crate::llm_commands::LlmEnvEntry],
) -> Result<String, CliError> {
    let argv = split_synthesizer_command(command_line)?;
    let mut command = Command::new(&argv[0]);
    for entry in env {
        command.env(&entry.key, &entry.value);
    }
    command.args(&argv[1..]).arg(prompt);
    let output = command.output().map_err(|error| {
        CliError::Core(format!(
            "failed to run synthesizer `{command_line}`: {error}"
        ))
    })?;
    if !output.status.success() {
        return Err(CliError::Core(format!(
            "synthesizer failed with status {}: {}",
            format_exit_status(&output.status),
            stderr_summary(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(crate) fn split_synthesizer_command(command_line: &str) -> Result<Vec<String>, CliError> {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Quote {
        Single,
        Double,
    }

    let mut argv = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut arg_started = false;
    let mut chars = command_line.chars().peekable();

    while let Some(ch) = chars.next() {
        match quote {
            Some(Quote::Single) => {
                if ch == '\'' {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            Some(Quote::Double) => match ch {
                '"' => quote = None,
                '\\' => {
                    let Some(next) = chars.peek().copied() else {
                        return Err(CliError::InvalidArgument(
                            "unterminated escape in --synthesizer".to_string(),
                        ));
                    };
                    if matches!(next, '"' | '\\' | '$' | '`' | '\n') {
                        current.push(chars.next().expect("peeked synthesizer char"));
                    } else {
                        current.push(ch);
                    }
                }
                _ => current.push(ch),
            },
            None => match ch {
                ch if ch.is_whitespace() => {
                    if arg_started {
                        argv.push(std::mem::take(&mut current));
                        arg_started = false;
                    }
                }
                '\'' => {
                    quote = Some(Quote::Single);
                    arg_started = true;
                }
                '"' => {
                    quote = Some(Quote::Double);
                    arg_started = true;
                }
                '\\' => {
                    let Some(next) = chars.next() else {
                        return Err(CliError::InvalidArgument(
                            "unterminated escape in --synthesizer".to_string(),
                        ));
                    };
                    current.push(next);
                    arg_started = true;
                }
                _ => {
                    current.push(ch);
                    arg_started = true;
                }
            },
        }
    }

    if quote.is_some() {
        return Err(CliError::InvalidArgument(
            "unterminated quote in --synthesizer".to_string(),
        ));
    }
    if arg_started {
        argv.push(current);
    }
    if argv.is_empty() {
        return Err(CliError::InvalidArgument(
            "--synthesizer must not be empty".to_string(),
        ));
    }
    Ok(argv)
}

pub(crate) fn strip_markdown_fence(candidate: &str) -> String {
    let trimmed = candidate.trim();
    let mut lines = trimmed.lines().collect::<Vec<_>>();
    if lines
        .first()
        .is_some_and(|line| line.trim_start().starts_with("```"))
    {
        lines.remove(0);
        if lines
            .last()
            .is_some_and(|line| line.trim_start().starts_with("```"))
        {
            lines.pop();
        }
        return lines.join("\n").trim().to_string();
    }
    trimmed.to_string()
}

fn synth_script_path(project_root: &Path, name: &str) -> PathBuf {
    project_root
        .join(".agentflow")
        .join("synth")
        .join(format!("{name}.py"))
}

fn auto_synth_fixture_path(project_root: &Path, name: &str) -> PathBuf {
    project_root
        .join(".agentflow")
        .join("synth")
        .join(format!("{name}.fixture.txt"))
}

fn auto_synth_alternate_fixture_path(project_root: &Path, name: &str) -> PathBuf {
    project_root
        .join(".agentflow")
        .join("synth")
        .join(format!("{name}.alt.fixture.txt"))
}

fn cleanup_auto_synth_candidate(script_path: &Path, fixture_paths: &[&Path]) {
    let _ = fs::remove_file(script_path);
    for fixture_path in fixture_paths {
        let _ = fs::remove_file(fixture_path);
    }
}

fn validate_candidate_script(
    script_path: &Path,
    fixture: &Path,
) -> Result<ValidationOutput, CliError> {
    validate_candidate_script_with_inputs(
        script_path,
        fixture,
        SynthValidationInputs {
            gene: PRIMARY_VALIDATION_GENE,
        },
    )
}

fn validate_candidate_script_with_inputs(
    script_path: &Path,
    fixture: &Path,
    inputs: SynthValidationInputs<'_>,
) -> Result<ValidationOutput, CliError> {
    let workdir = isolated_workdir()?;
    fs::create_dir_all(&workdir)?;
    let result = run_python_script(
        script_path,
        Some(fixture),
        &workdir,
        VALIDATION_TIMEOUT,
        inputs,
    );
    let _ = fs::remove_dir_all(&workdir);
    result
}

fn validate_runtime_candidate_script(
    script_path: &Path,
    runtime_gene: &str,
) -> Result<ValidationOutput, CliError> {
    let workdir = isolated_workdir()?;
    fs::create_dir_all(&workdir)?;
    let result = run_python_script(
        script_path,
        None,
        &workdir,
        VALIDATION_TIMEOUT,
        SynthValidationInputs { gene: runtime_gene },
    );
    let _ = fs::remove_dir_all(&workdir);
    result
}

fn run_python_script(
    script_path: &Path,
    fixture: Option<&Path>,
    workdir: &Path,
    timeout: Duration,
    inputs: SynthValidationInputs<'_>,
) -> Result<ValidationOutput, CliError> {
    let result_path = workdir.join("result.txt");
    let mut command = Command::new("/usr/bin/env");
    command
        .env_clear()
        .env("PATH", VALIDATION_PATH)
        .env("PYTHONPATH", cbioportal_pythonpath_value())
        .env("AGENTFLOW_WORKDIR", workdir)
        .env("AGENTFLOW_OUTPUT_RESULT", &result_path)
        .arg("python3")
        .arg(script_path)
        .current_dir(workdir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    set_validation_domain_param_env(&mut command, inputs);
    if let Some(fixture) = fixture {
        command.env("SYNTH_INPUT", fixture);
    }
    configure_child_process_group(&mut command);
    let mut child = command.spawn().map_err(|error| {
        CliError::Core(format!(
            "failed to run candidate script {}: {error}",
            script_path.display()
        ))
    })?;
    let started = SystemTime::now();

    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            let result_output = fs::read_to_string(&result_path).ok();
            return Ok(ValidationOutput {
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                exit_code: output.status.code(),
                timed_out: false,
                result_output,
            });
        }

        if started.elapsed().unwrap_or_default() >= timeout {
            kill_child_process_group(&mut child);
            let output = child.wait_with_output()?;
            let result_output = fs::read_to_string(&result_path).ok();
            return Ok(ValidationOutput {
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                exit_code: output.status.code(),
                timed_out: true,
                result_output,
            });
        }

        thread::sleep(Duration::from_millis(20));
    }
}

fn set_validation_domain_param_env(command: &mut Command, inputs: SynthValidationInputs<'_>) {
    for param in DEFAULT_SYNTH_DOMAIN_PARAMS {
        let Some(value) = validation_value_for_domain_param(param, inputs) else {
            continue;
        };
        command.env(synth_domain_param_env_name(param.name), value);
    }
}

fn validation_value_for_domain_param<'a>(
    param: &SynthDomainParam,
    inputs: SynthValidationInputs<'a>,
) -> Option<&'a str> {
    match param.name {
        "gene" => Some(inputs.gene),
        _ => None,
    }
}

fn synth_domain_param_env_name(name: &str) -> String {
    let key = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("AGENTFLOW_PARAM_{key}")
}

fn isolated_workdir() -> Result<PathBuf, CliError> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(std::env::temp_dir().join(format!("agentflow-synth-{}-{nanos}", std::process::id())))
}

fn synthesized_tool_yaml(name: &str, description: &str, script_path: &Path) -> String {
    let description = yaml_single_line(description);
    let maturity = ToolMaturity::Exploratory.as_str();
    let params_yaml = synthesized_params_yaml(DEFAULT_SYNTH_DOMAIN_PARAMS);
    let pythonpath_env_arg = cbioportal_pythonpath_env_arg();
    format!(
        r#"schema_version: {}
namespace: synth
name: {}
version: {}
maturity: {}
description: {}
{}outputs:
  result:
    type: Text
runtime:
  backend: local
  command:
    - /usr/bin/env
    - {}
    - python3
    - {}
"#,
        agentflow_schemas::TOOL_SCHEMA_V0,
        name,
        SYNTH_VERSION,
        maturity,
        description,
        params_yaml,
        pythonpath_env_arg,
        script_path.display()
    )
}

fn synthesized_agent_tool_yaml(name: &str, description: &str, script_path: &Path) -> String {
    let description = yaml_single_line(description);
    let maturity = ToolMaturity::Exploratory.as_str();
    let params_yaml = synthesized_params_yaml(DEFAULT_SYNTH_DOMAIN_PARAMS);
    let pythonpath_env_arg = cbioportal_pythonpath_env_arg();
    format!(
        r#"schema_version: {}
namespace: synth
name: {}
version: {}
maturity: {}
description: {}
{}outputs:
  result:
    type: Markdown
    observer: artifact_summary
runtime:
  backend: local
  timeout_seconds: 60
  command:
    - /usr/bin/env
    - {}
    - python3
    - {}
"#,
        agentflow_schemas::TOOL_SCHEMA_V0,
        name,
        SYNTH_VERSION,
        maturity,
        description,
        params_yaml,
        pythonpath_env_arg,
        script_path.display()
    )
}

fn cbioportal_client_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("AGENTFLOW_CBIOPORTAL_CLIENT_DIR") {
        return PathBuf::from(dir);
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(CBIOPORTAL_CLIENT_RELATIVE_DIR);
    fs::canonicalize(&dir).unwrap_or(dir)
}

fn cbioportal_pythonpath_value() -> String {
    cbioportal_client_dir().display().to_string()
}

fn cbioportal_pythonpath_env_arg() -> String {
    format!("PYTHONPATH={}", cbioportal_pythonpath_value())
}

fn synthesized_params_yaml(params: &[SynthDomainParam]) -> String {
    if params.is_empty() {
        return String::new();
    }

    let entries = params
        .iter()
        .map(|param| {
            format!(
                "  {}:\n    type: {}\n    required: {}\n",
                param.name, param.type_name, param.required
            )
        })
        .collect::<String>();
    format!("params:\n{entries}")
}

fn validate_input_sensitivity(
    primary: &ValidationOutput,
    alternate: &ValidationOutput,
    fixture_paths: &[&Path],
) -> Result<(), String> {
    let primary_output =
        normalize_output_for_sensitivity(validation_result_text(primary), fixture_paths);
    let alternate_output =
        normalize_output_for_sensitivity(validation_result_text(alternate), fixture_paths);
    if primary_output == alternate_output {
        return Err(format!(
            concat!(
                "candidate failed input sensitivity: normalized outputs were identical ",
                "across distinct fixtures; primary={}, alternate={}"
            ),
            snippet(validation_result_text(primary)),
            snippet(validation_result_text(alternate))
        ));
    }
    Ok(())
}

fn validate_runtime_gate_behavior(
    runtime: &ValidationOutput,
    primary: &ValidationOutput,
    alternate: &ValidationOutput,
    fixture_paths: &[&Path],
) -> Result<(), String> {
    if runtime.timed_out {
        return Err(format!(
            "candidate failed runtime gate: timed_out=true, stdout={}, stderr={}",
            snippet(&runtime.stdout),
            snippet(&runtime.stderr)
        ));
    }

    if runtime.exit_code != Some(0) {
        return Err(format!(
            "candidate failed runtime gate: exit_code={}, stdout={}, stderr={}",
            runtime
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            snippet(&runtime.stdout),
            snippet(&runtime.stderr)
        ));
    }

    if validation_result_text(runtime).trim().is_empty() {
        return Err("candidate failed runtime gate: exited 0 without output".to_string());
    }

    let runtime_output =
        normalize_output_for_sensitivity(validation_result_text(runtime), fixture_paths);
    let primary_output =
        normalize_output_for_sensitivity(validation_result_text(primary), fixture_paths);
    let alternate_output =
        normalize_output_for_sensitivity(validation_result_text(alternate), fixture_paths);
    if runtime_output == primary_output || runtime_output == alternate_output {
        return Err(format!(
            concat!(
                "candidate failed runtime gate: output matched fixture smoke output, ",
                "suggesting a default fallback; stdout={}, stderr={}"
            ),
            snippet(&runtime.stdout),
            snippet(&runtime.stderr)
        ));
    }

    Ok(())
}

fn validation_result_text(validation: &ValidationOutput) -> &str {
    validation
        .result_output
        .as_deref()
        .filter(|output| !output.trim().is_empty())
        .unwrap_or(&validation.stdout)
}

fn normalize_fixture_for_comparison(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_output_for_sensitivity(value: &str, volatile_paths: &[&Path]) -> String {
    let mut normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    for path in volatile_paths {
        normalized = replace_path_variants(&normalized, path);
    }

    normalized
        .lines()
        .map(|line| {
            line.split_whitespace()
                .map(mask_volatile_token)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn replace_path_variants(value: &str, path: &Path) -> String {
    let mut normalized = value.replace(&path.display().to_string(), "<input-path>");
    if let Some(file_name) = path.file_name().and_then(|name| name.to_str()) {
        normalized = normalized.replace(file_name, "<input-file>");
    }
    normalized
}

fn mask_volatile_token(token: &str) -> String {
    if contains_iso_date(token) || contains_clock_time(token) || is_epoch_like(token) {
        "<volatile>".to_string()
    } else {
        token.to_string()
    }
}

fn contains_iso_date(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.windows(10).any(|window| {
        window[0].is_ascii_digit()
            && window[1].is_ascii_digit()
            && window[2].is_ascii_digit()
            && window[3].is_ascii_digit()
            && window[4] == b'-'
            && window[5].is_ascii_digit()
            && window[6].is_ascii_digit()
            && window[7] == b'-'
            && window[8].is_ascii_digit()
            && window[9].is_ascii_digit()
    })
}

fn contains_clock_time(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.windows(5).any(|window| {
        window[0].is_ascii_digit()
            && window[1].is_ascii_digit()
            && window[2] == b':'
            && window[3].is_ascii_digit()
            && window[4].is_ascii_digit()
    })
}

fn is_epoch_like(token: &str) -> bool {
    let trimmed = token.trim_matches(|ch: char| !ch.is_ascii_alphanumeric());
    trimmed.len() >= 10 && trimmed.bytes().all(|byte| byte.is_ascii_digit())
}

fn auto_synth_validation_passed(validation: &ValidationOutput, expect: &str) -> bool {
    validation.exit_code == Some(0)
        && !validation.timed_out
        && validation.stdout.trim().contains(expect)
        && validation
            .result_output
            .as_deref()
            .is_some_and(|output| !output.trim().is_empty() && output.contains(expect))
}

fn auto_synth_rejection_reason(phase: &str, validation: &ValidationOutput, expect: &str) -> String {
    format!(
        concat!(
            "candidate failed {}: ",
            "exit_code={}, timed_out={}, expected={}, stdout={}, stderr={}"
        ),
        phase,
        validation
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        validation.timed_out,
        expect,
        snippet(&validation.stdout),
        snippet(&validation.stderr)
    )
}

fn snippet(value: &str) -> String {
    let trimmed = value.trim();
    let boundary = trimmed
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= 240)
        .last()
        .unwrap_or(0);
    if trimmed.len() <= 240 {
        trimmed.to_string()
    } else {
        format!("{}…", &trimmed[..boundary])
    }
}

fn configure_child_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
}

fn kill_child_process_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .arg("-TERM")
            .arg(format!("-{}", child.id()))
            .status();
    }
    let _ = child.kill();
}

fn auto_synth_tool_name(
    hypothesis_statement: &str,
    capability_need: &str,
) -> Result<String, CliError> {
    let mut slug = String::new();
    let mut previous_separator = false;
    for ch in hypothesis_statement
        .chars()
        .chain(std::iter::once(' '))
        .chain(capability_need.chars())
    {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_separator = false;
        } else if !previous_separator && !slug.is_empty() {
            slug.push('_');
            previous_separator = true;
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_matches('_');
    let slug = if slug.is_empty() { "tool" } else { slug };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!("auto_synth_{slug}_{:x}", nanos % 0xffff_ffff);
    validate_tool_name(&name)?;
    Ok(name)
}

fn yaml_single_line(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\n' | '\r' | '\t' | '#' | ':' => ' ',
            ch => ch,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_exit_status(status: &ExitStatus) -> String {
    status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn stderr_summary(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "no stderr".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use agentflow_core::storage::FlowDraft;

    use super::*;

    fn args(items: Vec<String>) -> Vec<OsString> {
        items.into_iter().map(OsString::from).collect()
    }

    fn temp_project_path(test_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "agentflow-cli-synth-{test_name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn init_project(path: &Path) {
        crate::run(args(vec![
            "agentflow".to_string(),
            "init".to_string(),
            "--name".to_string(),
            "Synth Demo".to_string(),
            "--path".to_string(),
            path.display().to_string(),
        ]))
        .unwrap();
    }

    fn write_fixture(path: &Path, contents: &str) -> PathBuf {
        let fixture = path.join("fixture.txt");
        fs::write(&fixture, contents).unwrap();
        fixture
    }

    fn write_stub_synthesizer(path: &Path, name: &str, candidate: &str) -> PathBuf {
        let stub = path.join(name);
        fs::write(
            &stub,
            format!(
                r#"#!/bin/sh
cat <<'PY'
{candidate}
PY
"#
            ),
        )
        .unwrap();
        stub
    }

    fn write_retrying_auto_synthesizer(
        path: &Path,
        name: &str,
        first_candidate: &str,
        repaired_candidate: &str,
        prompt_log: &Path,
    ) -> PathBuf {
        let stub = path.join(name);
        fs::write(
            &stub,
            format!(
                r#"import pathlib
import sys

prompt = sys.argv[1] if len(sys.argv) > 1 else ""
with open(r"{prompt_log}", "a", encoding="utf-8") as handle:
    handle.write("\n---PROMPT---\n")
    handle.write(prompt)

if "study not found: lihc_tcga" in prompt and "修正它" in prompt:
    print(r'''{repaired_candidate}''')
else:
    print(r'''{first_candidate}''')
"#,
                prompt_log = prompt_log.display()
            ),
        )
        .unwrap();
        stub
    }

    fn synth_args(
        path: &Path,
        fixture: &Path,
        synthesizer: &str,
        name: &str,
        expect: &str,
    ) -> Vec<OsString> {
        args(vec![
            "agentflow".to_string(),
            "synth".to_string(),
            "--name".to_string(),
            name.to_string(),
            "--description".to_string(),
            "Echo the input file exactly".to_string(),
            "--fixture".to_string(),
            fixture.display().to_string(),
            "--expect".to_string(),
            expect.to_string(),
            "--synthesizer".to_string(),
            synthesizer.to_string(),
            "--path".to_string(),
            path.display().to_string(),
        ])
    }

    #[test]
    fn cbioportal_client_self_test_passes_without_network() {
        let client = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/tools/agentflow_cbioportal.py");

        let output = std::process::Command::new("/usr/bin/env")
            .arg("python3")
            .arg(&client)
            .arg("--self-test")
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "client self-test failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn split_synthesizer_command_respects_shell_quotes() {
        assert_eq!(
            split_synthesizer_command("/bin/sh '/tmp/project with space/synth.sh' --flag").unwrap(),
            vec![
                "/bin/sh".to_string(),
                "/tmp/project with space/synth.sh".to_string(),
                "--flag".to_string()
            ]
        );
        assert_eq!(
            split_synthesizer_command(r#"python3 "two words.py""#).unwrap(),
            vec!["python3".to_string(), "two words.py".to_string()]
        );
    }

    #[test]
    fn split_synthesizer_command_rejects_unterminated_quotes() {
        let error = split_synthesizer_command("python3 'missing-end").unwrap_err();
        assert!(error.message().contains("unterminated quote"));
    }

    #[test]
    fn synth_validates_and_registers_exploratory_tool() {
        let path = temp_project_path("validated");
        init_project(&path);
        let fixture = write_fixture(&path, "expected-line\n");
        let candidate = r#"import os
from pathlib import Path
print(Path(os.environ["SYNTH_INPUT"]).read_text(), end="")"#;
        let stub = write_stub_synthesizer(&path, "stub_good.sh", candidate);
        let synthesizer = format!("/bin/sh {}", stub.display());

        let output = crate::run(synth_args(
            &path,
            &fixture,
            &synthesizer,
            "echo_input",
            "expected-line",
        ))
        .unwrap();

        assert!(output.contains("VALIDATED"));
        assert!(output.contains("synth/echo_input"));
        assert!(output.contains("exploratory"));

        let list = crate::run(args(vec![
            "agentflow".to_string(),
            "tools".to_string(),
            "list".to_string(),
            "--path".to_string(),
            path.display().to_string(),
        ]))
        .unwrap();
        assert!(list.contains("synth/echo_input@0.1.0 [exploratory]"));

        let inspect = crate::run(args(vec![
            "agentflow".to_string(),
            "tools".to_string(),
            "inspect".to_string(),
            "synth/echo_input".to_string(),
            "--json".to_string(),
            "--path".to_string(),
            path.display().to_string(),
        ]))
        .unwrap();
        assert!(inspect.contains("\"maturity\":\"exploratory\""));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn synth_rejects_unvalidated_script_without_registering() {
        let path = temp_project_path("rejected");
        init_project(&path);
        let fixture = write_fixture(&path, "expected-line\n");
        let stub = write_stub_synthesizer(&path, "stub_bad.sh", r#"print("wrong output")"#);
        let synthesizer = format!("/bin/sh {}", stub.display());

        let output = crate::run(synth_args(
            &path,
            &fixture,
            &synthesizer,
            "bad_echo",
            "expected-line",
        ))
        .unwrap();

        assert!(output.contains("REJECTED"));
        assert!(output.contains("wrong output"));
        assert!(path.join(".agentflow/synth/bad_echo.py").exists());

        let list = crate::run(args(vec![
            "agentflow".to_string(),
            "tools".to_string(),
            "list".to_string(),
            "--path".to_string(),
            path.display().to_string(),
        ]))
        .unwrap();
        assert_eq!(list, "No tools registered");

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_rejects_failed_smoke_without_leaving_script() {
        let path = temp_project_path("auto-rejected");
        init_project(&path);
        let store = ProjectStore::open(&path).unwrap();
        let candidate = r#"===SCRIPT===
print("wrong output")
===FIXTURE===
fixture,line
===ALT_FIXTURE===
fixture,other-line
===EXPECT===
expected-line
"#;
        let stub = write_stub_synthesizer(&path, "stub_auto_bad.sh", candidate);
        let synthesizer = format!("/bin/sh {}", stub.display());

        let outcome = auto_synthesize_agent_tool(
            &store,
            &synthesizer,
            "Auto synth cleanup hypothesis",
            "Need a custom rejected tool",
            None,
        )
        .unwrap();

        match outcome {
            AutoSynthToolResult::Rejected(reason) => assert!(reason.contains("fixture smoke")),
            AutoSynthToolResult::Registered(tool_ref) => {
                panic!("unexpected auto-synth registration: {tool_ref}")
            }
            other => panic!("unexpected sourced auto-synth result: {other:?}"),
        }
        let synth_entries = fs::read_dir(path.join(".agentflow/synth"))
            .map(|entries| entries.count())
            .unwrap_or_default();
        assert_eq!(synth_entries, 0);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_rejects_input_invariant_script_without_leaving_script() {
        let path = temp_project_path("auto-invariant-rejected");
        init_project(&path);
        let store = ProjectStore::open(&path).unwrap();
        let candidate = r##"===SCRIPT===
import os
from pathlib import Path

_ = Path(os.environ["SYNTH_INPUT"]).read_text()
result = "# Auto synth report\nAUTO_SYNTH_OK\nhardcoded_hr=1.42\n"
output_path = os.environ.get("AGENTFLOW_OUTPUT_RESULT")
if output_path:
    Path(output_path).write_text(result, encoding="utf-8")
print(result, end="")
===FIXTURE===
cohort,hr
A,1.11
===ALT_FIXTURE===
cohort,hr
B,9.99
===EXPECT===
AUTO_SYNTH_OK
"##;
        let stub = write_stub_synthesizer(&path, "stub_auto_invariant.sh", candidate);
        let synthesizer = format!("/bin/sh {}", stub.display());

        let outcome = auto_synthesize_agent_tool(
            &store,
            &synthesizer,
            "Auto synth invariant hypothesis",
            "Need a custom input-sensitive tool",
            None,
        )
        .unwrap();

        match outcome {
            AutoSynthToolResult::Rejected(reason) => assert!(reason.contains("input sensitivity")),
            AutoSynthToolResult::Registered(tool_ref) => {
                panic!("unexpected auto-synth registration: {tool_ref}")
            }
            other => panic!("unexpected sourced auto-synth result: {other:?}"),
        }
        let synth_entries = fs::read_dir(path.join(".agentflow/synth"))
            .map(|entries| entries.count())
            .unwrap_or_default();
        assert_eq!(synth_entries, 0);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_rejects_input_sensitive_script_that_fails_runtime_gate() {
        let path = temp_project_path("auto-sensitive-runtime-rejected");
        init_project(&path);
        let store = ProjectStore::open(&path).unwrap();
        let candidate = r##"===SCRIPT===
import os
from pathlib import Path

input_path = os.environ.get("SYNTH_INPUT")
if not input_path:
    raise SystemExit("real data source unavailable")
lines = Path(input_path).read_text(encoding="utf-8").strip().splitlines()
value = lines[-1].split(",")[-1]
result = f"# Auto synth report\nAUTO_SYNTH_OK\nsource_value={value}\n"
output_path = os.environ.get("AGENTFLOW_OUTPUT_RESULT")
if output_path:
    Path(output_path).write_text(result, encoding="utf-8")
print(result, end="")
===FIXTURE===
cohort,value
A,1.11
===ALT_FIXTURE===
cohort,value
B,9.99
===EXPECT===
AUTO_SYNTH_OK
"##;
        let stub = write_stub_synthesizer(&path, "stub_auto_sensitive.sh", candidate);
        let synthesizer = format!("/bin/sh {}", stub.display());

        let outcome = auto_synthesize_agent_tool(
            &store,
            &synthesizer,
            "Auto synth sensitive hypothesis",
            "Need a custom input-sensitive tool",
            Some("MID1IP1"),
        )
        .unwrap();

        match outcome {
            AutoSynthToolResult::Rejected(reason) => {
                assert!(reason.contains("runtime gate"));
                assert!(reason.contains("real data source unavailable"));
            }
            AutoSynthToolResult::Registered(tool_ref) => {
                panic!("runtime-failing candidate should not register: {tool_ref}")
            }
            other => panic!("unexpected sourced auto-synth result: {other:?}"),
        }

        let tools = store.list_tools().unwrap();
        assert_eq!(tools.len(), 0);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_runtime_gate_failure_feeds_error_back_and_registers_repair() {
        let path = temp_project_path("auto-runtime-retry");
        init_project(&path);
        let store = ProjectStore::open(&path).unwrap();
        let first_candidate = r##"===SCRIPT===
import os
from pathlib import Path

def emit(result):
    output_path = os.environ.get("AGENTFLOW_OUTPUT_RESULT")
    if not output_path:
        raise SystemExit("AGENTFLOW_OUTPUT_RESULT is required")
    Path(output_path).write_text(result, encoding="utf-8")
    print(result, end="")

input_path = os.environ.get("SYNTH_INPUT")
if input_path:
    value = Path(input_path).read_text(encoding="utf-8").strip().splitlines()[-1]
    emit(f"# Auto synth report\nAUTO_SYNTH_OK\nfixture={value}\n")
else:
    raise SystemExit("study not found: lihc_tcga")
===FIXTURE===
primary,1.11
===ALT_FIXTURE===
alternate,9.99
===EXPECT===
AUTO_SYNTH_OK
"##;
        let repaired_candidate = r##"===SCRIPT===
import os
from pathlib import Path

def emit(result):
    output_path = os.environ.get("AGENTFLOW_OUTPUT_RESULT")
    if not output_path:
        raise SystemExit("AGENTFLOW_OUTPUT_RESULT is required")
    Path(output_path).write_text(result, encoding="utf-8")
    print(result, end="")

input_path = os.environ.get("SYNTH_INPUT")
if input_path:
    value = Path(input_path).read_text(encoding="utf-8").strip().splitlines()[-1]
    emit(f"# Auto synth report\nAUTO_SYNTH_OK\nfixture={value}\n")
else:
    gene = os.environ.get("AGENTFLOW_PARAM_GENE")
    if gene != "MID1IP1":
        raise SystemExit(f"unexpected runtime gene {gene}")
    emit(f"# Auto synth report\nAUTO_SYNTH_OK\ngene={gene}\nsource=runtime\n")
===FIXTURE===
primary,1.11
===ALT_FIXTURE===
alternate,9.99
===EXPECT===
AUTO_SYNTH_OK
"##;
        let prompt_log = path.join("auto-synth-prompts.log");
        let stub = write_retrying_auto_synthesizer(
            &path,
            "stub_auto_retry.py",
            first_candidate,
            repaired_candidate,
            &prompt_log,
        );
        let synthesizer = format!("/usr/bin/env python3 {}", stub.display());

        let outcome = auto_synthesize_agent_tool(
            &store,
            &synthesizer,
            "MID1IP1 immunotherapy biomarker claim",
            "Need a gene-specific public-data association tool",
            Some("MID1IP1"),
        )
        .unwrap();

        let tool_ref = match outcome {
            AutoSynthToolResult::Registered(tool_ref) => tool_ref,
            AutoSynthToolResult::Rejected(reason) => {
                panic!("repaired candidate should register: {reason}")
            }
            other => panic!("unexpected sourced auto-synth result: {other:?}"),
        };
        assert!(tool_ref.starts_with("synth/auto_synth_"));
        assert_eq!(store.list_tools().unwrap().len(), 1);

        let prompts = fs::read_to_string(&prompt_log).unwrap();
        assert_eq!(prompts.matches("---PROMPT---").count(), 3);
        assert!(prompts.contains("Return ONLY a JSON array"));
        assert!(prompts.contains("probe_url"));
        assert!(prompts.contains("study not found: lihc_tcga"));
        assert!(prompts.contains("修正它"));
        assert!(prompts.contains("raise SystemExit(\"study not found: lihc_tcga\")"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_rejects_after_three_runtime_gate_failures_without_registering() {
        let path = temp_project_path("auto-runtime-all-fail");
        init_project(&path);
        let store = ProjectStore::open(&path).unwrap();
        let candidate = r##"===SCRIPT===
import os
from pathlib import Path

input_path = os.environ.get("SYNTH_INPUT")
output_path = os.environ.get("AGENTFLOW_OUTPUT_RESULT")
if not output_path:
    raise SystemExit("AGENTFLOW_OUTPUT_RESULT is required")
if input_path:
    value = Path(input_path).read_text(encoding="utf-8").strip().splitlines()[-1]
    result = f"# Auto synth report\nAUTO_SYNTH_OK\nfixture={value}\n"
    Path(output_path).write_text(result, encoding="utf-8")
    print(result, end="")
else:
    raise SystemExit("permanent runtime error")
===FIXTURE===
primary,1.11
===ALT_FIXTURE===
alternate,9.99
===EXPECT===
AUTO_SYNTH_OK
"##;
        let stub = write_stub_synthesizer(&path, "stub_auto_all_fail.sh", candidate);
        let synthesizer = format!("/bin/sh {}", stub.display());

        let outcome = auto_synthesize_agent_tool(
            &store,
            &synthesizer,
            "MID1IP1 immunotherapy biomarker claim",
            "Need a gene-specific public-data association tool",
            Some("MID1IP1"),
        )
        .unwrap();

        match outcome {
            AutoSynthToolResult::Rejected(reason) => {
                assert!(reason.contains("runtime gate"));
                assert!(reason.contains("permanent runtime error"));
            }
            AutoSynthToolResult::Registered(tool_ref) => {
                panic!("all-failing candidate should not register: {tool_ref}")
            }
            other => panic!("unexpected sourced auto-synth result: {other:?}"),
        }
        assert!(store.list_tools().unwrap().is_empty());
        let synth_entries = fs::read_dir(path.join(".agentflow/synth"))
            .map(|entries| entries.count())
            .unwrap_or_default();
        assert_eq!(synth_entries, 0);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_accepts_dual_mode_script_requiring_gene_during_fixture_validation() {
        let path = temp_project_path("auto-gene-fixture-env");
        init_project(&path);
        let store = ProjectStore::open(&path).unwrap();
        let candidate = r##"===SCRIPT===
import os
from pathlib import Path

gene = os.environ.get("AGENTFLOW_PARAM_GENE")
output_path = os.environ.get("AGENTFLOW_OUTPUT_RESULT")
if not gene or not output_path:
    raise SystemExit("ERROR: AGENTFLOW_PARAM_GENE and AGENTFLOW_OUTPUT_RESULT must be set")

def emit(gene, source):
    result = f"# Auto synth report\nAUTO_SYNTH_OK\ngene={gene}\nsource={source}\n"
    Path(output_path).write_text(result, encoding="utf-8")
    print(result, end="")

input_path = os.environ.get("SYNTH_INPUT")
if not input_path:
    emit(gene, "runtime")
else:
    expected_gene, fixture_value = Path(input_path).read_text(encoding="utf-8").strip().split(",", 1)
    if gene != expected_gene:
        raise SystemExit(f"expected validation gene {expected_gene}, got {gene}")
    result = f"# Auto synth report\nAUTO_SYNTH_OK\ngene={gene}\nfixture={fixture_value}\n"
    Path(output_path).write_text(result, encoding="utf-8")
    print(result, end="")
===FIXTURE===
TP53,primary_fixture
===ALT_FIXTURE===
EGFR,alternate_fixture
===EXPECT===
AUTO_SYNTH_OK
"##;
        let stub = write_stub_synthesizer(&path, "stub_auto_gene_fixture_env.sh", candidate);
        let synthesizer = format!("/bin/sh {}", stub.display());

        let outcome = auto_synthesize_agent_tool(
            &store,
            &synthesizer,
            "TP53 and EGFR fixture-backed biomarker claim",
            "Need a gene-specific dual-mode tool",
            Some("TP53"),
        )
        .unwrap();

        match outcome {
            AutoSynthToolResult::Registered(tool_ref) => {
                assert!(tool_ref.starts_with("synth/auto_synth_"));
            }
            AutoSynthToolResult::Rejected(reason) => {
                panic!("gene-requiring fixture candidate should register: {reason}")
            }
            other => panic!("unexpected sourced auto-synth result: {other:?}"),
        }

        let tools = store.list_tools().unwrap();
        assert_eq!(tools.len(), 1);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_declares_gene_param_and_runtime_uses_agentflow_param_gene() {
        let path = temp_project_path("auto-gene-runtime");
        init_project(&path);
        let store = ProjectStore::open(&path).unwrap();
        let candidate = r##"===SCRIPT===
import os
from pathlib import Path

def emit(gene, source):
    result = f"# Auto synth report\nAUTO_SYNTH_OK\ngene={gene}\nsource={source}\n"
    output_path = os.environ.get("AGENTFLOW_OUTPUT_RESULT")
    if not output_path:
        raise SystemExit("AGENTFLOW_OUTPUT_RESULT is required")
    Path(output_path).write_text(result, encoding="utf-8")
    print(result, end="")

input_path = os.environ.get("SYNTH_INPUT")
if input_path:
    lines = Path(input_path).read_text(encoding="utf-8").strip().splitlines()
    gene = lines[-1].split(",")[0]
    emit(gene, "fixture")
else:
    gene = os.environ.get("AGENTFLOW_PARAM_GENE")
    if not gene:
        raise SystemExit("AGENTFLOW_PARAM_GENE is required")
    emit(gene, "runtime")
===FIXTURE===
gene,value
MID1IP1,1.11
===ALT_FIXTURE===
gene,value
KRAS,9.99
===EXPECT===
AUTO_SYNTH_OK
"##;
        let stub = write_stub_synthesizer(&path, "stub_auto_gene_runtime.sh", candidate);
        let synthesizer = format!("/bin/sh {}", stub.display());

        let tool_ref = match auto_synthesize_agent_tool(
            &store,
            &synthesizer,
            "MID1IP1 immunotherapy biomarker claim",
            "Need a gene-specific public-data association tool",
            Some("MID1IP1"),
        )
        .unwrap()
        {
            AutoSynthToolResult::Registered(tool_ref) => tool_ref,
            AutoSynthToolResult::Rejected(reason) => {
                panic!("dual-mode candidate should register: {reason}")
            }
            other => panic!("unexpected sourced auto-synth result: {other:?}"),
        };

        let inspection = store.inspect_tool(&tool_ref).unwrap();
        assert!(inspection
            .spec_json
            .contains("\"param_types\":{\"gene\":\"string\"}"));
        assert!(inspection
            .spec_json
            .contains("\"required_params\":[\"gene\"]"));
        assert!(inspection.spec_json.contains("\"gene\""));
        assert!(!inspection.spec_json.contains("\"input\""));

        let flow = FlowDraft::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.flow.v0
id: auto_gene_runtime
name: Auto gene runtime
steps:
  - id: run_gene
    tool: {tool_ref}
    needs: []
    params:
      gene: MID1IP1
    outputs:
      result: runtime_result
"#
        ))
        .unwrap();
        store.approve_flow(flow, None).unwrap();
        let summary = store.run_flow("auto_gene_runtime").unwrap();
        assert_eq!(summary.completed_steps, 1);
        assert_eq!(summary.failed_steps, 0);

        let computed = store
            .list_artifacts()
            .unwrap()
            .into_iter()
            .filter(|artifact| artifact.kind == "computed")
            .collect::<Vec<_>>();
        assert_eq!(computed.len(), 1);
        let output = fs::read_to_string(&computed[0].path).unwrap();
        assert!(output.contains("gene=MID1IP1"));
        assert!(output.contains("source=runtime"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_cbioportal_client_import_works_in_validation_and_registered_runtime() {
        let path = temp_project_path("auto-cbio-client-import");
        init_project(&path);
        let store = ProjectStore::open(&path).unwrap();
        let candidate = r##"===SCRIPT===
import os
from pathlib import Path
import agentflow_cbioportal as cbio

cbio.resolve_study = lambda cancer_keyword: "lihc_tcga_pan_can_atlas_2018"
cbio.fetch_expression = lambda study_id, gene: {
    f"{gene}_S1": 1.0,
    f"{gene}_S2": 3.5,
}
cbio.fetch_overall_survival = lambda study_id: {
    f"{study_id}_P1": (12.0, True),
    f"{study_id}_P2": (30.0, False),
}

def emit(text):
    output_path = os.environ.get("AGENTFLOW_OUTPUT_RESULT")
    if not output_path:
        raise SystemExit("AGENTFLOW_OUTPUT_RESULT is required")
    Path(output_path).write_text(text, encoding="utf-8")
    print(text, end="")

gene = os.environ.get("AGENTFLOW_PARAM_GENE")
if not gene:
    raise SystemExit("AGENTFLOW_PARAM_GENE is required")
mode = "runtime"
fixture_value = "none"
input_path = os.environ.get("SYNTH_INPUT")
if input_path:
    mode = "fixture"
    fixture_value = Path(input_path).read_text(encoding="utf-8").strip()
study = cbio.resolve_study("liver hepatocellular carcinoma")
expr = cbio.fetch_expression(study, gene)
surv = cbio.fetch_overall_survival(study)
emit(
    "# Auto synth report\n"
    "CBIO_STUB_OK\n"
    f"mode={mode}\n"
    f"fixture={fixture_value}\n"
    f"gene={gene}\n"
    f"study={study}\n"
    f"expression_samples={len(expr)}\n"
    f"survival_patients={len(surv)}\n"
)
===FIXTURE===
primary fixture value
===ALT_FIXTURE===
alternate fixture value
===EXPECT===
CBIO_STUB_OK
"##;
        let stub = write_stub_synthesizer(&path, "stub_auto_cbio_client.sh", candidate);
        let synthesizer = format!("/bin/sh {}", stub.display());

        let tool_ref = match auto_synthesize_agent_tool(
            &store,
            &synthesizer,
            "MID1IP1 expression survival association in hepatocellular carcinoma",
            "Need a cBioPortal-backed expression-survival association tool",
            Some("MID1IP1"),
        )
        .unwrap()
        {
            AutoSynthToolResult::Registered(tool_ref) => tool_ref,
            AutoSynthToolResult::Rejected(reason) => {
                panic!("client-importing candidate should register: {reason}")
            }
            other => panic!("unexpected sourced auto-synth result: {other:?}"),
        };

        let inspection = store.inspect_tool(&tool_ref).unwrap();
        assert!(inspection.spec_json.contains("PYTHONPATH="));

        let flow = FlowDraft::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.flow.v0
id: auto_cbio_runtime
name: Auto cBio runtime
steps:
  - id: run_cbio
    tool: {tool_ref}
    needs: []
    params:
      gene: MID1IP1
    outputs:
      result: runtime_result
"#
        ))
        .unwrap();
        store.approve_flow(flow, None).unwrap();
        let summary = store.run_flow("auto_cbio_runtime").unwrap();
        assert_eq!(summary.completed_steps, 1);
        assert_eq!(summary.failed_steps, 0);

        let computed = store
            .list_artifacts()
            .unwrap()
            .into_iter()
            .filter(|artifact| artifact.kind == "computed")
            .collect::<Vec<_>>();
        assert_eq!(computed.len(), 1);
        let output = fs::read_to_string(&computed[0].path).unwrap();
        assert!(output.contains("CBIO_STUB_OK"));
        assert!(output.contains("mode=runtime"));
        assert!(output.contains("gene=MID1IP1"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn auto_synth_prompt_forbids_default_or_illustrative_fallbacks() {
        let prompt = build_auto_synth_prompt(
            "MID1IP1 immunotherapy biomarker claim",
            "Need survival and immune-correlation evidence",
            None,
        );

        assert!(prompt.contains("禁止硬编码"));
        assert!(prompt.contains("真实数据不可得"));
        assert!(prompt.contains("非零退出"));
        assert!(prompt.contains("default"));
        assert!(prompt.contains("illustrative"));
        assert!(prompt.contains("===ALT_FIXTURE==="));
        assert!(prompt.contains("AGENTFLOW_PARAM_GENE"));
        assert!(prompt.contains("https://www.cbioportal.org/api"));
        assert!(prompt.contains("SYNTH_INPUT"));
        assert!(prompt.contains("tcga_survival_assoc.py"));
    }

    #[test]
    fn auto_synth_prompt_directs_llm_to_use_verified_cbioportal_client() {
        let prompt = build_auto_synth_prompt(
            "MID1IP1 expression survival association in hepatocellular carcinoma",
            "Need expression-survival association evidence",
            Some("Discovered real cBioPortal identifiers: studyId=lihc_tcga_pan_can_atlas_2018, mrnaMolecularProfileId=lihc_tcga_pan_can_atlas_2018_rna_seq_v2_mrna, sampleListId=lihc_tcga_pan_can_atlas_2018_all, api_base=https://www.cbioportal.org/api. Use these EXACT identifiers; do not guess."),
        );

        assert!(prompt.contains("import agentflow_cbioportal"));
        assert!(prompt.contains("resolve_study(cancer_keyword) -> str"));
        assert!(prompt.contains("fetch_expression(study_id, gene) -> dict"));
        assert!(prompt.contains("fetch_overall_survival(study_id) -> dict"));
        assert!(prompt.contains("do not write HTTP/API calls"));
        assert!(prompt.contains("only write analysis logic"));
        assert!(prompt.contains("AGENTFLOW_PARAM_GENE"));
        assert!(prompt.contains("AGENTFLOW_OUTPUT_RESULT"));
        assert!(prompt.contains("SYNTH_INPUT"));
        assert!(prompt.contains("Discovered real cBioPortal identifiers"));
    }

    #[test]
    fn cbioportal_grounding_discovers_liver_pan_can_atlas_identifiers() {
        let grounding = discover_cbioportal_grounding_with_fetcher(
            "High THRSP expression is protective in hepatocellular carcinoma",
            CBIOPORTAL_API_BASE,
            |url| match url {
                "https://www.cbioportal.org/api/studies" => Some(
                    r#"[
                      {"studyId":"brca_tcga_pan_can_atlas_2018","name":"Breast Cancer (TCGA, PanCancer Atlas)","description":"Breast carcinoma","cancerTypeId":"brca"},
                      {"studyId":"lihc_tcga","name":"Liver Hepatocellular Carcinoma (TCGA, Firehose Legacy)","description":"LIHC legacy cohort","cancerTypeId":"lihc"},
                      {"studyId":"lihc_tcga_pan_can_atlas_2018","name":"Liver Hepatocellular Carcinoma (TCGA, PanCancer Atlas)","description":"Hepatocellular carcinoma","cancerTypeId":"lihc"}
                    ]"#
                    .to_string(),
                ),
                "https://www.cbioportal.org/api/studies/lihc_tcga_pan_can_atlas_2018/molecular-profiles" => Some(
                    r#"[
                      {"molecularProfileId":"lihc_tcga_pan_can_atlas_2018_mutations","molecularAlterationType":"MUTATION_EXTENDED","datatype":"MAF","name":"Mutations"},
                      {"molecularProfileId":"lihc_tcga_pan_can_atlas_2018_rna_seq_v2_mrna","molecularAlterationType":"MRNA_EXPRESSION","datatype":"CONTINUOUS","name":"mRNA expression (RNA Seq V2 RSEM)"}
                    ]"#
                    .to_string(),
                ),
                "https://www.cbioportal.org/api/studies/lihc_tcga_pan_can_atlas_2018/sample-lists" => Some(
                    r#"[
                      {"sampleListId":"lihc_tcga_pan_can_atlas_2018_sequenced","category":"all_cases_with_mutation_and_cna_data","name":"Sequenced tumors"},
                      {"sampleListId":"lihc_tcga_pan_can_atlas_2018_all","category":"all_cases_in_study","name":"All samples"}
                    ]"#
                    .to_string(),
                ),
                _ => None,
            },
        )
        .expect("stubbed cBioPortal endpoints should ground LIHC");

        assert_eq!(
            grounding,
            "Discovered real cBioPortal identifiers: studyId=lihc_tcga_pan_can_atlas_2018, mrnaMolecularProfileId=lihc_tcga_pan_can_atlas_2018_rna_seq_v2_mrna, sampleListId=lihc_tcga_pan_can_atlas_2018_all, api_base=https://www.cbioportal.org/api. Use these EXACT identifiers; do not guess."
        );
    }

    #[test]
    fn cbioportal_grounding_returns_none_on_failure_or_no_match() {
        let fetch_failure = discover_cbioportal_grounding_with_fetcher(
            "High THRSP expression is protective in hepatocellular carcinoma",
            CBIOPORTAL_API_BASE,
            |_url| None,
        );
        assert!(fetch_failure.is_none());

        let no_match = discover_cbioportal_grounding_with_fetcher(
            "High THRSP expression is protective in hepatocellular carcinoma",
            CBIOPORTAL_API_BASE,
            |url| {
                match url {
                "https://www.cbioportal.org/api/studies" => Some(
                    r#"[
                      {"studyId":"brca_tcga_pan_can_atlas_2018","name":"Breast Cancer (TCGA, PanCancer Atlas)","description":"Breast carcinoma","cancerTypeId":"brca"}
                    ]"#
                    .to_string(),
                ),
                _ => None,
            }
            },
        );
        assert!(no_match.is_none());
    }

    #[test]
    fn auto_synth_prompt_injects_grounding_when_available() {
        let grounding = discover_cbioportal_grounding_with_fetcher(
            "High THRSP expression is protective in hepatocellular carcinoma",
            CBIOPORTAL_API_BASE,
            |url| {
                match url {
                "https://www.cbioportal.org/api/studies" => Some(
                    r#"[
                      {"studyId":"lihc_tcga_pan_can_atlas_2018","name":"Liver Hepatocellular Carcinoma (TCGA, PanCancer Atlas)","description":"Hepatocellular carcinoma","cancerTypeId":"lihc"}
                    ]"#
                    .to_string(),
                ),
                "https://www.cbioportal.org/api/studies/lihc_tcga_pan_can_atlas_2018/molecular-profiles" => Some(
                    r#"[
                      {"molecularProfileId":"lihc_tcga_pan_can_atlas_2018_rna_seq_v2_mrna","molecularAlterationType":"MRNA_EXPRESSION","datatype":"CONTINUOUS","name":"mRNA expression"}
                    ]"#
                    .to_string(),
                ),
                "https://www.cbioportal.org/api/studies/lihc_tcga_pan_can_atlas_2018/sample-lists" => Some(
                    r#"[
                      {"sampleListId":"lihc_tcga_pan_can_atlas_2018_all","category":"all_cases_in_study","name":"All samples"}
                    ]"#
                    .to_string(),
                ),
                _ => None,
            }
            },
        );
        let prompt = build_auto_synth_prompt(
            "High THRSP expression is protective in hepatocellular carcinoma",
            "Need expression-survival association evidence",
            grounding.as_deref(),
        );

        let grounding_pos = prompt
            .find("Discovered real cBioPortal identifiers")
            .expect("prompt should include discovered identifiers");
        let hypothesis_pos = prompt
            .find("Research hypothesis:")
            .expect("prompt should include hypothesis marker");
        assert!(grounding_pos < hypothesis_pos);
        assert!(prompt.contains(
            "studyId=lihc_tcga_pan_can_atlas_2018, mrnaMolecularProfileId=lihc_tcga_pan_can_atlas_2018_rna_seq_v2_mrna, sampleListId=lihc_tcga_pan_can_atlas_2018_all"
        ));
        assert!(prompt.contains("Use these EXACT identifiers; do not guess."));
    }

    #[test]
    fn autonomous_source_prompt_requests_allowlisted_json_candidates() {
        let prompt = build_source_discovery_prompt(
            "MID1IP1 immunotherapy response biomarker in melanoma",
            "Need public ICB cohort response data",
        );

        assert!(prompt.contains("JSON array"));
        assert!(prompt.contains("\"name\""));
        assert!(prompt.contains("\"base_url\""));
        assert!(prompt.contains("\"probe_url\""));
        assert!(prompt.contains("\"access_note\""));
        assert!(prompt.contains("\"required_data\""));
        assert!(prompt.contains("\"has_required_data\""));
        assert!(prompt.contains("\"required_data_reason\""));
        assert!(prompt.contains("回答该假设需要什么数据"));
        assert!(prompt.contains("read-only GET"));
        assert!(prompt.contains("https://www.ncbi.nlm.nih.gov"));
        assert!(prompt.contains("https://www.ebi.ac.uk"));
        assert!(prompt.contains("https://www.cbioportal.org"));
        assert!(prompt.contains("localhost"));
        assert!(prompt.contains("private IP"));
    }

    #[test]
    fn source_probe_safety_blocks_illegal_domains_schemes_and_private_hosts() {
        assert!(
            source_probe_safety("https://www.ncbi.nlm.nih.gov/geo/query/acc.cgi?acc=GSE1").is_ok()
        );
        assert!(source_probe_safety("https://www.cbioportal.org/api/studies").is_ok());

        let illegal_domain =
            source_probe_safety("https://evil.example.org/data?gene=MID1IP1").unwrap_err();
        assert!(illegal_domain.contains("not allowlisted"));

        let localhost = source_probe_safety("http://localhost:8000/probe").unwrap_err();
        assert!(localhost.contains("localhost"));

        let private_ip = source_probe_safety("http://10.0.0.2/probe").unwrap_err();
        assert!(private_ip.contains("private or local"));

        let file_scheme = source_probe_safety("file:///tmp/data.json").unwrap_err();
        assert!(file_scheme.contains("http(s)"));
    }

    #[test]
    fn python_fetch_scripts_disable_redirect_following() {
        for script in [CBIOPORTAL_DISCOVERY_FETCH_PY, SOURCE_PROBE_FETCH_PY] {
            assert!(script.contains("HTTPRedirectHandler"));
            assert!(script.contains("redirect_request"));
            assert!(script.contains("build_opener(_NoRedirect)"));
            assert!(script.contains("_opener.open(request, timeout=timeout)"));
        }
    }

    #[test]
    fn autonomous_source_discovery_selects_viable_source_and_injects_probe_summary() {
        let raw_candidates = r#"[
          {
            "name": "blocked mirror",
            "base_url": "https://evil.example.org",
            "probe_url": "https://evil.example.org/search?q=MID1IP1",
            "access_note": "not public science allowlist"
          },
          {
            "name": "NCBI GEO",
            "base_url": "https://www.ncbi.nlm.nih.gov",
            "probe_url": "https://www.ncbi.nlm.nih.gov/geo/query/acc.cgi?term=MID1IP1+immunotherapy",
            "access_note": "public GEO discovery page"
          }
        ]"#;

        let discovery = discover_sources_from_candidate_json(
            "MID1IP1 immunotherapy response biomarker",
            "Need public ICB cohort response data",
            raw_candidates,
            |url, _timeout| {
                assert!(url.contains("ncbi.nlm.nih.gov"));
                Some(
                    "GEO record mentions MID1IP1 gene expression in an immunotherapy response cohort"
                        .to_string(),
                )
            },
        );

        let viable = discovery.first_viable().expect("NCBI should be viable");
        assert_eq!(viable.candidate.name, "NCBI GEO");
        assert!(discovery.trace.contains("blocked mirror"));
        assert!(discovery.trace.contains("skipped"));
        assert!(discovery.trace.contains("viable"));

        let prompt = build_public_source_auto_synth_prompt(
            "MID1IP1 immunotherapy response biomarker",
            "Need public ICB cohort response data",
            viable,
            &discovery.trace,
        );
        assert!(prompt.contains("NCBI GEO"));
        assert!(prompt.contains("https://www.ncbi.nlm.nih.gov/geo/query/acc.cgi"));
        assert!(prompt.contains("GEO record mentions MID1IP1"));
        assert!(prompt.contains("read-only GET"));
        assert!(prompt.contains("If the public source lacks usable real data, exit non-zero"));
        assert!(!prompt.contains("import agentflow_cbioportal as cbio"));
    }

    #[test]
    fn source_discovery_probe_budget_skips_extra_safe_candidates() {
        let raw_candidates = r#"[
          {
            "name": "Blocked mirror",
            "base_url": "https://evil.example.org",
            "probe_url": "https://evil.example.org/search?q=MID1IP1",
            "access_note": "not public science allowlist"
          },
          {
            "name": "NCBI 1",
            "base_url": "https://www.ncbi.nlm.nih.gov",
            "probe_url": "https://www.ncbi.nlm.nih.gov/probe1",
            "access_note": "public source"
          },
          {
            "name": "NCBI 2",
            "base_url": "https://www.ncbi.nlm.nih.gov",
            "probe_url": "https://www.ncbi.nlm.nih.gov/probe2",
            "access_note": "public source"
          },
          {
            "name": "NCBI 3",
            "base_url": "https://www.ncbi.nlm.nih.gov",
            "probe_url": "https://www.ncbi.nlm.nih.gov/probe3",
            "access_note": "public source"
          },
          {
            "name": "NCBI 4",
            "base_url": "https://www.ncbi.nlm.nih.gov",
            "probe_url": "https://www.ncbi.nlm.nih.gov/probe4",
            "access_note": "public source"
          },
          {
            "name": "NCBI 5",
            "base_url": "https://www.ncbi.nlm.nih.gov",
            "probe_url": "https://www.ncbi.nlm.nih.gov/probe5",
            "access_note": "public source"
          },
          {
            "name": "NCBI 6",
            "base_url": "https://www.ncbi.nlm.nih.gov",
            "probe_url": "https://www.ncbi.nlm.nih.gov/probe6",
            "access_note": "public source"
          }
        ]"#;
        let mut probed = Vec::new();

        let discovery = discover_sources_from_candidate_json(
            "MID1IP1 immunotherapy response biomarker",
            "Need public ICB cohort response data",
            raw_candidates,
            |url, _timeout| {
                probed.push(url.to_string());
                Some("unrelated public index".to_string())
            },
        );

        assert_eq!(probed.len(), 5);
        assert!(probed.iter().all(|url| url.contains("ncbi.nlm.nih.gov")));
        assert!(discovery.trace.contains("candidate proposals parsed: 7"));
        assert!(discovery.trace.contains("Blocked mirror: skipped"));
        assert!(discovery
            .trace
            .contains("NCBI 6: skipped (probe budget 5 reached)"));
    }

    #[test]
    fn question_aware_source_discovery_marks_gene_only_source_insufficient() {
        let raw_candidates = r#"[
          {
            "name": "cBioPortal",
            "base_url": "https://www.cbioportal.org",
            "probe_url": "https://www.cbioportal.org/api/studies?projection=SUMMARY",
            "access_note": "public cBioPortal API with expression and survival studies",
            "required_data": "ICB-treated cohort with response labels and gene expression",
            "has_required_data": "no",
            "required_data_reason": "related expression/survival data, but no ICB treatment response labels"
          }
        ]"#;

        let discovery = discover_sources_from_candidate_json(
            "MID1IP1 immunotherapy response biomarker",
            "Need public ICB cohort response data with expression",
            raw_candidates,
            |_url, _timeout| {
                Some(
                    "cBioPortal TCGA melanoma studies include MID1IP1 mRNA expression and overall survival"
                        .to_string(),
                )
            },
        );

        assert!(discovery.first_viable().is_none());
        assert!(discovery.trace.contains("QUESTION DATA REQUIREMENTS"));
        assert!(discovery
            .trace
            .contains("ICB-treated cohort with response labels and gene expression"));
        assert!(discovery.trace.contains("cBioPortal"));
        assert!(discovery.trace.contains("related-but-insufficient"));
        assert!(discovery.trace.contains("has_required_data=no"));
        assert!(discovery.trace.contains("no ICB treatment response labels"));
        assert!(discovery.trace.contains("代理分析但不直接回答本问题"));
    }

    #[test]
    fn autonomous_source_discovery_has_honest_research_gap_when_no_viable_source() {
        let path = temp_project_path("source-discovery-no-viable");
        init_project(&path);
        let store = ProjectStore::open(&path).unwrap();
        let source_response = r#"[
          {
            "name": "Non allowlisted source",
            "base_url": "https://example.org",
            "probe_url": "https://example.org/probe?gene=MID1IP1",
            "access_note": "blocked"
          }
        ]"#;
        let stub = write_stub_synthesizer(&path, "stub_source_no_viable.sh", source_response);
        let synthesizer = format!("/bin/sh {}", stub.display());

        let outcome = auto_synthesize_agent_tool_with_fetcher(
            &store,
            &synthesizer,
            "MID1IP1 immunotherapy response biomarker",
            "Need public ICB cohort response data",
            Some("MID1IP1"),
            |_url, _timeout| panic!("blocked source must not be probed"),
        )
        .unwrap();

        match outcome {
            AutoSynthToolResult::RejectedWithSource {
                reason,
                source_trace,
                research_gap,
            } => {
                assert!(research_gap);
                assert!(reason.contains("未找到可访问公开数据源"));
                assert!(reason.contains("研究空白"));
                assert!(source_trace.contains("Non allowlisted source"));
            }
            AutoSynthToolResult::RegisteredWithSource { tool_ref, .. }
            | AutoSynthToolResult::Registered(tool_ref) => {
                panic!("no viable source should not register: {tool_ref}")
            }
            AutoSynthToolResult::Rejected(reason) => panic!("expected sourced rejection: {reason}"),
        }
        assert!(store.list_tools().unwrap().is_empty());

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn cbioportal_viable_source_uses_verified_client_prompt() {
        let raw_candidates = r#"[
          {
            "name": "cBioPortal",
            "base_url": "https://www.cbioportal.org",
            "probe_url": "https://www.cbioportal.org/api/studies?projection=SUMMARY",
            "access_note": "public cBioPortal API"
          }
        ]"#;
        let discovery = discover_sources_from_candidate_json(
            "MID1IP1 expression survival association in hepatocellular carcinoma",
            "Need expression-survival association evidence",
            raw_candidates,
            |_url, _timeout| Some("LIHC study mentions MID1IP1 expression data".to_string()),
        );
        let viable = discovery.first_viable().unwrap();

        let prompt = build_prompt_for_viable_source(
            "MID1IP1 expression survival association in hepatocellular carcinoma",
            "Need expression-survival association evidence",
            viable,
            &discovery.trace,
            Some("Discovered real cBioPortal identifiers: studyId=lihc_tcga_pan_can_atlas_2018, mrnaMolecularProfileId=lihc_tcga_pan_can_atlas_2018_rna_seq_v2_mrna, sampleListId=lihc_tcga_pan_can_atlas_2018_all, api_base=https://www.cbioportal.org/api. Use these EXACT identifiers; do not guess."),
        );

        assert!(prompt.contains("import agentflow_cbioportal as cbio"));
        assert!(prompt.contains("Discovered real cBioPortal identifiers"));
        assert!(prompt.contains("SOURCE DISCOVERY TRACE"));
        assert!(prompt.contains("cBioPortal"));
        assert!(prompt.contains("do not write HTTP/API calls"));
    }

    #[test]
    fn auto_synth_prompt_without_grounding_keeps_few_shot_contract() {
        let prompt = build_auto_synth_prompt(
            "MID1IP1 immunotherapy biomarker claim",
            "Need survival and immune-correlation evidence",
            None,
        );

        assert!(!prompt.contains("Discovered real cBioPortal identifiers"));
        assert!(prompt.contains("tcga_survival_assoc.py"));
        assert!(prompt.contains("===SCRIPT==="));
        assert!(prompt.contains("AGENTFLOW_PARAM_GENE"));
    }

    #[test]
    fn validation_env_does_not_inherit_host_home() {
        let path = temp_project_path("validation-env-clear");
        init_project(&path);
        let script = path.join("env_clear.py");
        fs::write(
            &script,
            r#"import os
from pathlib import Path

if os.environ.get("HOME"):
    raise SystemExit("HOME leaked into validation")
result = "ENV_CLEARED_OK\n"
output_path = os.environ.get("AGENTFLOW_OUTPUT_RESULT")
if output_path:
    Path(output_path).write_text(result, encoding="utf-8")
print(result, end="")
"#,
        )
        .unwrap();
        let fixture = write_fixture(&path, "unused\n");

        let validation = validate_candidate_script(&script, &fixture).unwrap();

        assert_eq!(validation.exit_code, Some(0), "{validation:?}");
        assert!(validation.stdout.contains("ENV_CLEARED_OK"));
        assert!(validation
            .result_output
            .as_deref()
            .is_some_and(|result| result.contains("ENV_CLEARED_OK")));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn synth_missing_synthesizer_errors_without_registering() {
        let path = temp_project_path("missing-synthesizer");
        init_project(&path);
        let fixture = write_fixture(&path, "expected-line\n");

        let error = crate::run(synth_args(
            &path,
            &fixture,
            "/definitely/missing-agentflow-synthesizer",
            "missing_backend",
            "expected-line",
        ))
        .unwrap_err();

        assert!(error.message().contains("failed to run synthesizer"));
        assert!(!path.join(".agentflow/synth/missing_backend.py").exists());

        let list = crate::run(args(vec![
            "agentflow".to_string(),
            "tools".to_string(),
            "list".to_string(),
            "--path".to_string(),
            path.display().to_string(),
        ]))
        .unwrap();
        assert_eq!(list, "No tools registered");

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn strip_markdown_fence_removes_python_fence() {
        let candidate = r#"```python
print("ok")
```"#;

        assert_eq!(strip_markdown_fence(candidate), "print(\"ok\")");
    }
}
