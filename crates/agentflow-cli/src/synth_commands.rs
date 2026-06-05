use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
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
const CBIOPORTAL_API_BASE: &str = "https://www.cbioportal.org/api";
const MAX_AUTO_SYNTH_ATTEMPTS: usize = 3;
const VALIDATION_PATH: &str = "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin";
const PRIMARY_VALIDATION_GENE: &str = "TP53";
const ALTERNATE_VALIDATION_GENE: &str = "EGFR";
const CBIOPORTAL_DISCOVERY_FETCH_PY: &str = r#"import json
import sys
import urllib.request

url = sys.argv[1]
timeout = float(sys.argv[2])
request = urllib.request.Request(url, headers={"Accept": "application/json"})
with urllib.request.urlopen(request, timeout=timeout) as response:
    body = response.read().decode("utf-8")
json.loads(body)
print(body, end="" if body.endswith("\n") else "\n")
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

pub(crate) enum AutoSynthToolResult {
    Registered(String),
    Rejected(String),
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
    let grounding = discover_cbioportal_grounding(hypothesis_statement);
    let base_prompt =
        build_auto_synth_prompt(hypothesis_statement, capability_need, grounding.as_deref());
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
                return Ok(AutoSynthToolResult::Registered(registration.tool_ref));
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

    Ok(AutoSynthToolResult::Rejected(last_rejection))
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
                    "When using cBioPortal, use the exact studyId, mRNA molecular profile id, sample list id, and api_base above. ",
                    "Do not substitute remembered identifiers.\n\n"
                ),
                block
            )
        })
        .unwrap_or_default();
    format!(
        concat!(
            "You are writing an AgentFlow exploratory analysis tool. Use only Python 3 standard library.\n",
            "{}",
            "The generated tool spec will declare a required domain parameter named gene, so runtime receives AGENTFLOW_PARAM_GENE.\n",
            "The tool must support two modes with the same calculation logic:\n",
            "1. Runtime mode: when SYNTH_INPUT is unset, read domain parameters from AGENTFLOW_PARAM_<UPPER_NAME>, especially AGENTFLOW_PARAM_GENE.\n",
            "   Fetch real data from public sources such as cBioPortal REST at https://www.cbioportal.org/api (other public sources are allowed).\n",
            "   Write the main Markdown/Text result to the path in AGENTFLOW_OUTPUT_RESULT and also print it to stdout.\n",
            "2. Validation mode: when SYNTH_INPUT is set, read that fixture file, run the same deterministic calculation logic offline, write AGENTFLOW_OUTPUT_RESULT, and print stdout.\n",
            "禁止硬编码或编造 HR、p-value、correlation、effect size、biomarker grade、sample count, or any other numeric/stance result.\n",
            "Do not use DEFAULT_PANEL, default, demo, sample, placeholder, illustrative, toy, or fallback conclusions.\n",
            "真实数据不可得时必须 loudly fail: print a clear error to stderr and 非零退出 (exit non-zero).\n",
            "Never silently succeed with default/illustrative fallback data.\n",
            "During validation, SYNTH_INPUT will point to two meaningfully different fixtures; AGENTFLOW_PARAM_GENE will be TP53 for the first fixture and EGFR for the second fixture; the normalized output must change when these inputs change.\n",
            "If neither SYNTH_INPUT fixture data nor runtime real public data is available, exit non-zero instead of inventing data.\n\n",
            "Few-shot runtime contract from examples/tools/tcga_survival_assoc.py, adapted to AGENTFLOW_OUTPUT_RESULT:\n",
            "import os, urllib.request, urllib.parse, json\n",
            "API = \"https://www.cbioportal.org/api\"\n",
            "gene = os.environ.get(\"AGENTFLOW_PARAM_GENE\")\n",
            "out = os.environ.get(\"AGENTFLOW_OUTPUT_RESULT\")\n",
            "if not gene or not out: raise SystemExit(\"AGENTFLOW_PARAM_GENE and AGENTFLOW_OUTPUT_RESULT are required\")\n",
            "url = API + \"/genes/\" + urllib.parse.quote(gene)\n",
            "with urllib.request.urlopen(url, timeout=60) as resp:\n",
            "    gene_info = json.load(resp)\n",
            "open(out, \"w\").write(\"Gene: %s\\nentrez: %s\\n\" % (gene, gene_info[\"entrezGeneId\"]))\n\n",
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
            "runtime mode must read AGENTFLOW_PARAM_GENE, fetch/use real data, write AGENTFLOW_OUTPUT_RESULT, ",
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
    - python3
    - {}
"#,
        agentflow_schemas::TOOL_SCHEMA_V0,
        name,
        SYNTH_VERSION,
        maturity,
        description,
        params_yaml,
        script_path.display()
    )
}

fn synthesized_agent_tool_yaml(name: &str, description: &str, script_path: &Path) -> String {
    let description = yaml_single_line(description);
    let maturity = ToolMaturity::Exploratory.as_str();
    let params_yaml = synthesized_params_yaml(DEFAULT_SYNTH_DOMAIN_PARAMS);
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
    - python3
    - {}
"#,
        agentflow_schemas::TOOL_SCHEMA_V0,
        name,
        SYNTH_VERSION,
        maturity,
        description,
        params_yaml,
        script_path.display()
    )
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
        };
        assert!(tool_ref.starts_with("synth/auto_synth_"));
        assert_eq!(store.list_tools().unwrap().len(), 1);

        let prompts = fs::read_to_string(&prompt_log).unwrap();
        assert_eq!(prompts.matches("---PROMPT---").count(), 2);
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
