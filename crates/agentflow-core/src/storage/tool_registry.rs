use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::{params, OptionalExtension};

use crate::domain::ToolMaturity;

use super::migrations;
use super::project_store::{now_unix_seconds, EventRecord, ProjectStore, StorageError};

const DEFAULT_NAMESPACE: &str = "local";
const SIMPLE_YAML_SOURCE_FORMAT: &str = "agentflow.tool.v0.simple_yaml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    pub schema_version: String,
    pub namespace: String,
    pub name: String,
    pub version: String,
    pub maturity: ToolMaturity,
    pub description: String,
    pub inputs: BTreeMap<String, ToolPortSpec>,
    pub params: BTreeMap<String, ToolParamSpec>,
    pub outputs: BTreeMap<String, ToolPortSpec>,
    pub runtime: ToolRuntimeSpec,
    pub source_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPortSpec {
    pub type_name: String,
    pub required: bool,
    pub observer: Option<String>,
    pub min_rows: Option<usize>,
    pub required_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolParamSpec {
    pub type_name: String,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRuntimeSpec {
    pub backend: String,
    pub command: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableToolSpec {
    pub tool_ref: String,
    pub version: String,
    pub inputs: BTreeMap<String, ToolPortSpec>,
    pub params: BTreeMap<String, ToolParamSpec>,
    pub outputs: BTreeMap<String, ToolPortSpec>,
    pub runtime: ToolRuntimeSpec,
}

impl ToolSpec {
    pub fn from_simple_yaml(source_text: &str) -> Result<Self, StorageError> {
        let fields = parse_top_level_fields(source_text);
        let schema_version = required_field(&fields, "schema_version")?;
        if schema_version != agentflow_schemas::TOOL_SCHEMA_V0 {
            return Err(StorageError::InvalidInput(format!(
                "tool schema_version must be {}",
                agentflow_schemas::TOOL_SCHEMA_V0
            )));
        }

        let namespace = fields
            .get("namespace")
            .cloned()
            .unwrap_or_else(|| DEFAULT_NAMESPACE.to_string());
        let name = required_field(&fields, "name")?;
        let version = required_field(&fields, "version")?;
        let maturity_name = required_field(&fields, "maturity")?;
        let description = required_field(&fields, "description")?;
        let maturity = ToolMaturity::parse(&maturity_name).ok_or_else(|| {
            StorageError::InvalidInput(format!(
                "maturity must be one of: verified, wrapped, exploratory; got {maturity_name}"
            ))
        })?;

        validate_ref_part("namespace", &namespace)?;
        validate_ref_part("name", &name)?;
        validate_ref_part("version", &version)?;
        let executable = parse_executable_sections(source_text)?;
        executable.validate()?;

        Ok(Self {
            schema_version,
            namespace,
            name,
            version,
            maturity,
            description,
            inputs: executable.inputs,
            params: executable.params,
            outputs: executable.outputs,
            runtime: executable.runtime,
            source_text: source_text.to_string(),
        })
    }

    pub fn tool_ref(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }

    fn tool_id(&self) -> String {
        tool_id(&self.namespace, &self.name)
    }

    fn version_id(&self) -> String {
        tool_version_id(&self.namespace, &self.name, &self.version)
    }

    fn stored_json(&self) -> String {
        let required_inputs = self
            .inputs
            .iter()
            .filter_map(|(name, port)| port.required.then_some(name.as_str()))
            .collect::<Vec<_>>();
        format!(
            concat!(
                "{{",
                "\"schema_version\":\"{}\",",
                "\"namespace\":\"{}\",",
                "\"name\":\"{}\",",
                "\"version\":\"{}\",",
                "\"maturity\":\"{}\",",
                "\"description\":\"{}\",",
                "\"input_types\":{},",
                "\"required_inputs\":{},",
                "\"param_types\":{},",
                "\"required_params\":{},",
                "\"output_types\":{},",
                "\"output_observers\":{},",
                "\"input_min_rows\":{},",
                "\"input_required_columns\":{},",
                "\"output_min_rows\":{},",
                "\"output_required_columns\":{},",
                "\"runtime_backend\":\"{}\",",
                "\"runtime_command\":{},",
                "\"source_format\":\"{}\",",
                "\"source_text\":\"{}\"",
                "}}"
            ),
            escape_json(&self.schema_version),
            escape_json(&self.namespace),
            escape_json(&self.name),
            escape_json(&self.version),
            self.maturity,
            escape_json(&self.description),
            type_map_json(&self.inputs),
            string_array_json(&required_inputs),
            param_type_map_json(&self.params),
            string_array_json(
                &self
                    .params
                    .iter()
                    .filter_map(|(name, param)| param.required.then_some(name.as_str()))
                    .collect::<Vec<_>>()
            ),
            type_map_json(&self.outputs),
            observer_map_json(&self.outputs),
            min_rows_map_json(&self.inputs),
            required_columns_map_json(&self.inputs),
            min_rows_map_json(&self.outputs),
            required_columns_map_json(&self.outputs),
            escape_json(&self.runtime.backend),
            string_array_json(
                &self
                    .runtime
                    .command
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            ),
            SIMPLE_YAML_SOURCE_FORMAT,
            escape_json(&self.source_text)
        )
    }
}

struct ParsedExecutableSections {
    inputs: BTreeMap<String, ToolPortSpec>,
    params: BTreeMap<String, ToolParamSpec>,
    outputs: BTreeMap<String, ToolPortSpec>,
    runtime: ToolRuntimeSpec,
}

impl ParsedExecutableSections {
    fn validate(&self) -> Result<(), StorageError> {
        if self.outputs.is_empty() {
            return Err(StorageError::InvalidInput(
                "tool spec must declare at least one output".to_string(),
            ));
        }
        if self.runtime.backend != "local" {
            return Err(StorageError::InvalidInput(
                "V0 runtime.backend must be local".to_string(),
            ));
        }
        if self.runtime.command.is_empty() {
            return Err(StorageError::InvalidInput(
                "tool spec runtime.command must contain at least one argv entry".to_string(),
            ));
        }
        let executable = Path::new(&self.runtime.command[0]);
        if !executable.is_absolute() {
            return Err(StorageError::InvalidInput(
                "runtime.command[0] must be an absolute executable path".to_string(),
            ));
        }
        if is_inline_interpreter_command(&self.runtime.command) {
            return Err(StorageError::InvalidInput(
                "runtime.command must not use shell/interpreter inline execution".to_string(),
            ));
        }
        for arg in &self.runtime.command {
            if arg.trim().is_empty() || arg.contains('\n') || arg.contains('\0') {
                return Err(StorageError::InvalidInput(
                    "runtime.command entries must be non-empty single argv values".to_string(),
                ));
            }
        }
        validate_ports("input", &self.inputs)?;
        validate_ports("output", &self.outputs)?;
        for (name, param) in &self.params {
            validate_ref_part("param name", name)?;
            if param.type_name.trim().is_empty() {
                return Err(StorageError::InvalidInput(format!(
                    "param {name} must declare type"
                )));
            }
        }
        Ok(())
    }
}

fn is_inline_interpreter_command(command: &[String]) -> bool {
    let Some(executable) = command.first() else {
        return false;
    };
    let Some(flag) = command.get(1).map(String::as_str) else {
        return false;
    };
    let basename = Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(executable);
    matches!(
        (basename, flag),
        ("sh" | "bash" | "zsh" | "fish", "-c" | "-lc")
            | ("python" | "python3" | "perl" | "ruby" | "node", "-c" | "-e")
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRegistration {
    pub tool_ref: String,
    pub version: String,
    pub spec_hash: String,
    pub replaced_existing_version: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSummary {
    pub id: String,
    pub namespace: String,
    pub name: String,
    pub latest_version: String,
    pub maturity: String,
}

impl ToolSummary {
    pub fn tool_ref(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInspection {
    pub summary: ToolSummary,
    pub version_id: String,
    pub version: String,
    pub schema_version: String,
    pub spec_json: String,
    pub spec_hash: String,
    pub created_at: i64,
}

impl ToolInspection {
    pub fn to_json(&self) -> String {
        format!(
            concat!(
                "{{",
                "\"schema_version\":\"{}\",",
                "\"tool\":{{",
                "\"ref\":\"{}\",",
                "\"namespace\":\"{}\",",
                "\"name\":\"{}\",",
                "\"latest_version\":\"{}\",",
                "\"maturity\":\"{}\"",
                "}},",
                "\"version\":{{",
                "\"id\":\"{}\",",
                "\"version\":\"{}\",",
                "\"schema_version\":\"{}\",",
                "\"spec_hash\":\"{}\",",
                "\"created_at\":{},",
                "\"spec\":{}",
                "}}",
                "}}"
            ),
            agentflow_schemas::TOOL_INSPECTION_JSON_SCHEMA_V0,
            escape_json(&self.summary.tool_ref()),
            escape_json(&self.summary.namespace),
            escape_json(&self.summary.name),
            escape_json(&self.summary.latest_version),
            escape_json(&self.summary.maturity),
            escape_json(&self.version_id),
            escape_json(&self.version),
            escape_json(&self.schema_version),
            escape_json(&self.spec_hash),
            self.created_at,
            self.spec_json
        )
    }
}

impl ProjectStore {
    pub fn register_tool(&self, spec: ToolSpec) -> Result<ToolRegistration, StorageError> {
        let stored_json = spec.stored_json();
        let spec_hash = migrations::checksum(&stored_json);
        let tool_id = spec.tool_id();
        let version_id = spec.version_id();
        let now = now_unix_seconds();

        let existing_version = self
            .connection()
            .query_row(
                "SELECT id FROM tool_versions WHERE id = ?1",
                params![&version_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let replaced_existing_version = existing_version.is_some();

        self.connection().execute(
            "INSERT INTO tools (id, name, namespace, latest_version, maturity)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               latest_version = excluded.latest_version,
               maturity = excluded.maturity",
            params![
                &tool_id,
                &spec.name,
                &spec.namespace,
                &spec.version,
                spec.maturity.as_str()
            ],
        )?;

        self.connection().execute(
            "INSERT INTO tool_versions
             (id, tool_id, version, schema_version, spec_json, spec_hash, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
               schema_version = excluded.schema_version,
               spec_json = excluded.spec_json,
               spec_hash = excluded.spec_hash",
            params![
                &version_id,
                &tool_id,
                &spec.version,
                &spec.schema_version,
                &stored_json,
                &spec_hash,
                now
            ],
        )?;

        self.append_event(EventRecord {
            flow_id: None,
            step_id: None,
            run_id: None,
            event_type: "tool_registered".to_string(),
            payload_json: format!(
                "{{\"tool_ref\":\"{}\",\"version\":\"{}\",\"spec_hash\":\"{}\"}}",
                escape_json(&spec.tool_ref()),
                escape_json(&spec.version),
                escape_json(&spec_hash)
            ),
        })?;
        self.touch_project()?;

        Ok(ToolRegistration {
            tool_ref: spec.tool_ref(),
            version: spec.version,
            spec_hash,
            replaced_existing_version,
        })
    }

    pub fn list_tools(&self) -> Result<Vec<ToolSummary>, StorageError> {
        let mut stmt = self.connection().prepare(
            "SELECT id, namespace, name, latest_version, maturity
             FROM tools
             ORDER BY namespace ASC, name ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ToolSummary {
                id: row.get(0)?,
                namespace: row.get(1)?,
                name: row.get(2)?,
                latest_version: row.get(3)?,
                maturity: row.get(4)?,
            })
        })?;

        let mut tools = Vec::new();
        for row in rows {
            tools.push(row?);
        }
        Ok(tools)
    }

    pub fn inspect_tool(&self, tool_ref: &str) -> Result<ToolInspection, StorageError> {
        let parsed = ParsedToolRef::parse(tool_ref)?;
        let id = tool_id(&parsed.namespace, &parsed.name);
        let summary = self
            .connection()
            .query_row(
                "SELECT id, namespace, name, latest_version, maturity
                 FROM tools
                 WHERE id = ?1",
                params![id],
                |row| {
                    Ok(ToolSummary {
                        id: row.get(0)?,
                        namespace: row.get(1)?,
                        name: row.get(2)?,
                        latest_version: row.get(3)?,
                        maturity: row.get(4)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(format!("tool {tool_ref}")))?;

        let version = parsed
            .version
            .clone()
            .unwrap_or_else(|| summary.latest_version.clone());
        let version_id = tool_version_id(&parsed.namespace, &parsed.name, &version);
        let (schema_version, spec_json, spec_hash, created_at) = self
            .connection()
            .query_row(
                "SELECT schema_version, spec_json, spec_hash, created_at
                 FROM tool_versions
                 WHERE id = ?1",
                params![version_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(format!("tool version {tool_ref}")))?;

        Ok(ToolInspection {
            summary,
            version_id,
            version,
            schema_version,
            spec_json,
            spec_hash,
            created_at,
        })
    }

    pub fn executable_tool(&self, tool_ref: &str) -> Result<ExecutableToolSpec, StorageError> {
        let inspection = self.inspect_tool(tool_ref)?;
        executable_from_stored_json(
            &inspection.summary.tool_ref(),
            &inspection.version,
            &inspection.spec_json,
        )
    }
}

struct ParsedToolRef {
    namespace: String,
    name: String,
    version: Option<String>,
}

impl ParsedToolRef {
    fn parse(input: &str) -> Result<Self, StorageError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(StorageError::InvalidInput(
                "tool ref must not be empty".to_string(),
            ));
        }

        let (name_part, version) = match trimmed.split_once('@') {
            Some((name_part, version)) => {
                validate_ref_part("version", version)?;
                (name_part, Some(version.to_string()))
            }
            None => (trimmed, None),
        };
        let (namespace, name) = match name_part.split_once('/') {
            Some((namespace, name)) => (namespace, name),
            None => (DEFAULT_NAMESPACE, name_part),
        };

        validate_ref_part("namespace", namespace)?;
        validate_ref_part("name", name)?;

        Ok(Self {
            namespace: namespace.to_string(),
            name: name.to_string(),
            version,
        })
    }
}

fn parse_top_level_fields(source_text: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();

    for line in source_text.lines() {
        let without_comment = line.split_once('#').map_or(line, |(left, _)| left);
        if without_comment.trim().is_empty() || line.starts_with(char::is_whitespace) {
            continue;
        }
        let Some((key, value)) = without_comment.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = normalize_scalar(value.trim());
        if !key.is_empty() && !value.is_empty() {
            fields.insert(key.to_string(), value);
        }
    }

    fields
}

fn parse_executable_sections(source_text: &str) -> Result<ParsedExecutableSections, StorageError> {
    let mut inputs: BTreeMap<String, ToolPortSpec> = BTreeMap::new();
    let mut params: BTreeMap<String, ToolParamSpec> = BTreeMap::new();
    let mut outputs: BTreeMap<String, ToolPortSpec> = BTreeMap::new();
    let mut runtime = ToolRuntimeSpec {
        backend: String::new(),
        command: Vec::new(),
    };
    let mut section: Option<String> = None;
    let mut item: Option<String> = None;
    let mut in_runtime_command = false;

    for line in source_text.lines() {
        let without_comment = line.split_once('#').map_or(line, |(left, _)| left);
        if without_comment.trim().is_empty() {
            continue;
        }
        let indent = without_comment
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .count();
        let trimmed = without_comment.trim();

        if indent == 0 {
            in_runtime_command = false;
            item = None;
            let key = trimmed
                .split_once(':')
                .map_or(trimmed, |(key, _)| key)
                .trim();
            section = match key {
                "inputs" | "params" | "outputs" | "runtime" => Some(key.to_string()),
                _ => None,
            };
            continue;
        }

        match section.as_deref() {
            Some("inputs") | Some("outputs") | Some("params") => {
                let section_name = section.as_deref().unwrap();
                if indent == 2 {
                    let key = trimmed.trim_end_matches(':').trim();
                    validate_ref_part("tool section item", key)?;
                    item = Some(key.to_string());
                    match section_name {
                        "inputs" => {
                            inputs.entry(key.to_string()).or_insert(ToolPortSpec {
                                type_name: String::new(),
                                required: true,
                                observer: None,
                                min_rows: None,
                                required_columns: Vec::new(),
                            });
                        }
                        "outputs" => {
                            outputs.entry(key.to_string()).or_insert(ToolPortSpec {
                                type_name: String::new(),
                                required: true,
                                observer: None,
                                min_rows: None,
                                required_columns: Vec::new(),
                            });
                        }
                        "params" => {
                            params.entry(key.to_string()).or_insert(ToolParamSpec {
                                type_name: String::new(),
                                required: false,
                            });
                        }
                        _ => {}
                    }
                    continue;
                }

                if indent >= 4 {
                    let Some(item_name) = item.as_deref() else {
                        return Err(StorageError::InvalidInput(format!(
                            "{section_name} field appears before item name"
                        )));
                    };
                    let Some((key, value)) = trimmed.split_once(':') else {
                        return Err(StorageError::InvalidInput(format!(
                            "cannot parse tool {section_name} line: {trimmed}"
                        )));
                    };
                    set_tool_item_field(
                        section_name,
                        item_name,
                        key.trim(),
                        normalize_scalar(value.trim()),
                        &mut inputs,
                        &mut params,
                        &mut outputs,
                    )?;
                }
            }
            Some("runtime") => {
                if indent == 2 {
                    let Some((key, value)) = trimmed.split_once(':') else {
                        return Err(StorageError::InvalidInput(format!(
                            "cannot parse runtime line: {trimmed}"
                        )));
                    };
                    match key.trim() {
                        "backend" => {
                            runtime.backend = normalize_scalar(value.trim());
                            in_runtime_command = false;
                        }
                        "command" => {
                            if !value.trim().is_empty() {
                                return Err(StorageError::InvalidInput(
                                    "runtime.command must be a nested argv list".to_string(),
                                ));
                            }
                            in_runtime_command = true;
                        }
                        other => {
                            return Err(StorageError::InvalidInput(format!(
                                "unsupported runtime field {other}"
                            )));
                        }
                    }
                    continue;
                }

                if indent >= 4 && in_runtime_command {
                    let Some(value) = trimmed.strip_prefix("- ") else {
                        return Err(StorageError::InvalidInput(format!(
                            "runtime.command entries must use - value syntax: {trimmed}"
                        )));
                    };
                    runtime.command.push(normalize_scalar(value));
                }
            }
            _ => {}
        }
    }

    Ok(ParsedExecutableSections {
        inputs,
        params,
        outputs,
        runtime,
    })
}

fn set_tool_item_field(
    section: &str,
    item_name: &str,
    key: &str,
    value: String,
    inputs: &mut BTreeMap<String, ToolPortSpec>,
    params: &mut BTreeMap<String, ToolParamSpec>,
    outputs: &mut BTreeMap<String, ToolPortSpec>,
) -> Result<(), StorageError> {
    match section {
        "inputs" => {
            let port = inputs.get_mut(item_name).ok_or_else(|| {
                StorageError::InvalidInput(format!("unknown input item {item_name}"))
            })?;
            match key {
                "type" => port.type_name = value,
                "required" => port.required = parse_bool(&value)?,
                "observer" => {
                    return Err(StorageError::InvalidInput(
                        "observer is only supported on output ports".to_string(),
                    ));
                }
                "min_rows" => port.min_rows = Some(parse_usize_field("min_rows", &value)?),
                "required_columns" => port.required_columns = parse_columns(&value)?,
                other => {
                    return Err(StorageError::InvalidInput(format!(
                        "unsupported input field {other}"
                    )));
                }
            }
        }
        "outputs" => {
            let port = outputs.get_mut(item_name).ok_or_else(|| {
                StorageError::InvalidInput(format!("unknown output item {item_name}"))
            })?;
            match key {
                "type" => port.type_name = value,
                "required" => port.required = parse_bool(&value)?,
                "observer" => {
                    validate_observer_adapter(&value)?;
                    port.observer = Some(value);
                }
                "min_rows" => port.min_rows = Some(parse_usize_field("min_rows", &value)?),
                "required_columns" => port.required_columns = parse_columns(&value)?,
                other => {
                    return Err(StorageError::InvalidInput(format!(
                        "unsupported output field {other}"
                    )));
                }
            }
        }
        "params" => {
            let param = params.get_mut(item_name).ok_or_else(|| {
                StorageError::InvalidInput(format!("unknown param item {item_name}"))
            })?;
            match key {
                "type" => param.type_name = value,
                "required" => param.required = parse_bool(&value)?,
                other => {
                    return Err(StorageError::InvalidInput(format!(
                        "unsupported param field {other}"
                    )));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn parse_bool(value: &str) -> Result<bool, StorageError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(StorageError::InvalidInput(format!(
            "expected boolean true or false, got {value}"
        ))),
    }
}

fn parse_usize_field(field: &str, value: &str) -> Result<usize, StorageError> {
    let parsed = value.parse::<usize>().map_err(|_| {
        StorageError::InvalidInput(format!(
            "{field} must be a non-negative integer, got {value}"
        ))
    })?;
    if parsed == 0 {
        return Err(StorageError::InvalidInput(format!(
            "{field} must be greater than zero"
        )));
    }
    Ok(parsed)
}

fn parse_columns(value: &str) -> Result<Vec<String>, StorageError> {
    let mut columns = Vec::new();
    for column in value.split(',') {
        let column = column.trim();
        if column.is_empty() {
            return Err(StorageError::InvalidInput(
                "required_columns must not contain empty column names".to_string(),
            ));
        }
        if column.contains('\n') || column.contains('\0') {
            return Err(StorageError::InvalidInput(
                "required_columns entries must be single-line text".to_string(),
            ));
        }
        columns.push(column.to_string());
    }
    if columns.is_empty() {
        return Err(StorageError::InvalidInput(
            "required_columns must list at least one column".to_string(),
        ));
    }
    Ok(columns)
}

fn validate_ports(label: &str, ports: &BTreeMap<String, ToolPortSpec>) -> Result<(), StorageError> {
    for (name, port) in ports {
        validate_ref_part(&format!("{label} name"), name)?;
        if port.type_name.trim().is_empty() {
            return Err(StorageError::InvalidInput(format!(
                "{label} {name} must declare type"
            )));
        }
        if let Some(observer) = port.observer.as_deref() {
            if label != "output" {
                return Err(StorageError::InvalidInput(format!(
                    "{label} {name} must not declare observer"
                )));
            }
            validate_observer_adapter(observer)?;
        }
    }
    Ok(())
}

fn validate_observer_adapter(observer: &str) -> Result<(), StorageError> {
    validate_ref_part("observer adapter", observer)?;
    match observer {
        "artifact_summary" | "marker_report" => Ok(()),
        other => Err(StorageError::InvalidInput(format!(
            "unsupported observer adapter {other}; supported adapters are artifact_summary and marker_report"
        ))),
    }
}

fn stored_min_rows(
    map: &BTreeMap<String, String>,
    name: &str,
) -> Result<Option<usize>, StorageError> {
    map.get(name)
        .map(|value| parse_usize_field("min_rows", value))
        .transpose()
}

fn stored_columns(map: &BTreeMap<String, String>, name: &str) -> Result<Vec<String>, StorageError> {
    map.get(name)
        .map(|value| parse_columns(value))
        .transpose()
        .map(Option::unwrap_or_default)
}

fn executable_from_stored_json(
    tool_ref: &str,
    version: &str,
    spec_json: &str,
) -> Result<ExecutableToolSpec, StorageError> {
    let input_types = extract_string_map(spec_json, "input_types")?;
    let required_inputs = extract_string_array(spec_json, "required_inputs")?
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let param_types = extract_string_map(spec_json, "param_types")?;
    let required_params = extract_string_array(spec_json, "required_params")?
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let output_types = extract_string_map(spec_json, "output_types")?;
    let output_observers = extract_optional_string_map(spec_json, "output_observers")?;
    let input_min_rows = extract_optional_string_map(spec_json, "input_min_rows")?;
    let input_required_columns = extract_optional_string_map(spec_json, "input_required_columns")?;
    let output_min_rows = extract_optional_string_map(spec_json, "output_min_rows")?;
    let output_required_columns =
        extract_optional_string_map(spec_json, "output_required_columns")?;
    let runtime = ToolRuntimeSpec {
        backend: extract_string_field(spec_json, "runtime_backend")?,
        command: extract_string_array(spec_json, "runtime_command")?,
    };

    let inputs = input_types
        .into_iter()
        .map(|(name, type_name)| {
            let required = required_inputs.contains(&name);
            let min_rows = stored_min_rows(&input_min_rows, &name)?;
            let required_columns = stored_columns(&input_required_columns, &name)?;
            Ok((
                name,
                ToolPortSpec {
                    type_name,
                    required,
                    observer: None,
                    min_rows,
                    required_columns,
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>, StorageError>>()?;
    let params = param_types
        .into_iter()
        .map(|(name, type_name)| {
            let required = required_params.contains(&name);
            (
                name,
                ToolParamSpec {
                    type_name,
                    required,
                },
            )
        })
        .collect();
    let outputs = output_types
        .into_iter()
        .map(|(name, type_name)| {
            let observer = output_observers.get(&name).cloned();
            let min_rows = stored_min_rows(&output_min_rows, &name)?;
            let required_columns = stored_columns(&output_required_columns, &name)?;
            Ok((
                name,
                ToolPortSpec {
                    type_name,
                    required: true,
                    observer,
                    min_rows,
                    required_columns,
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>, StorageError>>()?;

    let executable = ExecutableToolSpec {
        tool_ref: tool_ref.to_string(),
        version: version.to_string(),
        inputs,
        params,
        outputs,
        runtime,
    };
    ParsedExecutableSections {
        inputs: executable.inputs.clone(),
        params: executable.params.clone(),
        outputs: executable.outputs.clone(),
        runtime: executable.runtime.clone(),
    }
    .validate()?;
    Ok(executable)
}

fn required_field(
    fields: &BTreeMap<String, String>,
    field_name: &str,
) -> Result<String, StorageError> {
    fields.get(field_name).cloned().ok_or_else(|| {
        StorageError::InvalidInput(format!("tool spec is missing required field {field_name}"))
    })
}

fn normalize_scalar(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

fn validate_ref_part(label: &str, value: &str) -> Result<(), StorageError> {
    if value.trim().is_empty() {
        return Err(StorageError::InvalidInput(format!(
            "{label} must not be empty"
        )));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(StorageError::InvalidInput(format!(
            "{label} may only contain ASCII letters, numbers, underscore, dash, and dot"
        )));
    }
    Ok(())
}

fn tool_id(namespace: &str, name: &str) -> String {
    format!("tool:{namespace}/{name}")
}

fn tool_version_id(namespace: &str, name: &str, version: &str) -> String {
    format!("tool_version:{namespace}/{name}@{version}")
}

fn type_map_json(map: &BTreeMap<String, ToolPortSpec>) -> String {
    let fields = map
        .iter()
        .map(|(name, port)| {
            format!(
                "\"{}\":\"{}\"",
                escape_json(name),
                escape_json(&port.type_name)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{fields}}}")
}

fn observer_map_json(map: &BTreeMap<String, ToolPortSpec>) -> String {
    let fields = map
        .iter()
        .filter_map(|(name, port)| {
            port.observer
                .as_ref()
                .map(|observer| format!("\"{}\":\"{}\"", escape_json(name), escape_json(observer)))
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{fields}}}")
}

fn min_rows_map_json(map: &BTreeMap<String, ToolPortSpec>) -> String {
    let fields = map
        .iter()
        .filter_map(|(name, port)| {
            port.min_rows
                .map(|min_rows| format!("\"{}\":\"{}\"", escape_json(name), min_rows))
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{fields}}}")
}

fn required_columns_map_json(map: &BTreeMap<String, ToolPortSpec>) -> String {
    let fields = map
        .iter()
        .filter(|(_, port)| !port.required_columns.is_empty())
        .map(|(name, port)| {
            format!(
                "\"{}\":\"{}\"",
                escape_json(name),
                escape_json(&port.required_columns.join(","))
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{fields}}}")
}

fn param_type_map_json(map: &BTreeMap<String, ToolParamSpec>) -> String {
    let fields = map
        .iter()
        .map(|(name, param)| {
            format!(
                "\"{}\":\"{}\"",
                escape_json(name),
                escape_json(&param.type_name)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{fields}}}")
}

fn string_array_json(values: &[&str]) -> String {
    let items = values
        .iter()
        .map(|value| format!("\"{}\"", escape_json(value)))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
}

fn extract_string_field(json: &str, field: &str) -> Result<String, StorageError> {
    let marker = format!("\"{field}\":\"");
    let start = json.find(&marker).ok_or_else(|| {
        StorageError::InvalidInput(format!("stored tool spec is missing field {field}"))
    })? + marker.len();
    let rest = &json[start..];
    let end = find_json_string_end(rest)?;
    Ok(unescape_json_string(&rest[..end]))
}

fn extract_string_array(json: &str, field: &str) -> Result<Vec<String>, StorageError> {
    let marker = format!("\"{field}\":[");
    let start = json.find(&marker).ok_or_else(|| {
        StorageError::InvalidInput(format!("stored tool spec is missing array {field}"))
    })? + marker.len();
    let rest = &json[start..];
    let mut values = Vec::new();
    let mut index = 0;
    loop {
        index = skip_json_whitespace(rest, index);
        let Some(next) = rest[index..].chars().next() else {
            return Err(StorageError::InvalidInput(format!(
                "stored tool spec array {field} is malformed"
            )));
        };
        if next == ']' {
            return Ok(values);
        }
        if next != '"' {
            return Err(StorageError::InvalidInput(format!(
                "stored tool spec array {field} contains non-string item"
            )));
        }
        let string_start = index + 1;
        let string_end = string_start + find_json_string_end(&rest[string_start..])?;
        values.push(unescape_json_string(&rest[string_start..string_end]));
        index = string_end + 1;
        index = skip_json_whitespace(rest, index);
        let Some(separator) = rest[index..].chars().next() else {
            return Err(StorageError::InvalidInput(format!(
                "stored tool spec array {field} is malformed"
            )));
        };
        match separator {
            ',' => {
                index += 1;
            }
            ']' => return Ok(values),
            _ => {
                return Err(StorageError::InvalidInput(format!(
                    "stored tool spec array {field} is malformed"
                )));
            }
        }
    }
}

fn extract_string_map(json: &str, field: &str) -> Result<BTreeMap<String, String>, StorageError> {
    let marker = format!("\"{field}\":{{");
    let start = json.find(&marker).ok_or_else(|| {
        StorageError::InvalidInput(format!("stored tool spec is missing map {field}"))
    })? + marker.len();
    extract_string_map_after_marker(json, field, start)
}

fn extract_optional_string_map(
    json: &str,
    field: &str,
) -> Result<BTreeMap<String, String>, StorageError> {
    let marker = format!("\"{field}\":{{");
    let Some(start) = json.find(&marker).map(|index| index + marker.len()) else {
        return Ok(BTreeMap::new());
    };
    extract_string_map_after_marker(json, field, start)
}

fn extract_string_map_after_marker(
    json: &str,
    field: &str,
    start: usize,
) -> Result<BTreeMap<String, String>, StorageError> {
    let rest = &json[start..];
    let mut map = BTreeMap::new();
    let mut index = 0;
    loop {
        index = skip_json_whitespace(rest, index);
        let Some(next) = rest[index..].chars().next() else {
            return Err(StorageError::InvalidInput(format!(
                "stored tool spec map {field} is malformed"
            )));
        };
        if next == '}' {
            return Ok(map);
        }
        if next != '"' {
            return Err(StorageError::InvalidInput(format!(
                "stored tool spec map {field} contains non-string key"
            )));
        }
        let key_start = index + 1;
        let key_end = key_start + find_json_string_end(&rest[key_start..])?;
        let key = unescape_json_string(&rest[key_start..key_end]);
        index = skip_json_whitespace(rest, key_end + 1);
        if !rest[index..].starts_with(':') {
            return Err(StorageError::InvalidInput(format!(
                "stored tool spec map {field} is malformed"
            )));
        }
        index += 1;
        index = skip_json_whitespace(rest, index);
        if !rest[index..].starts_with('"') {
            return Err(StorageError::InvalidInput(format!(
                "stored tool spec map {field} contains non-string value"
            )));
        }
        let value_start = index + 1;
        let value_end = value_start + find_json_string_end(&rest[value_start..])?;
        let value = unescape_json_string(&rest[value_start..value_end]);
        map.insert(key, value);
        index = skip_json_whitespace(rest, value_end + 1);
        let Some(separator) = rest[index..].chars().next() else {
            return Err(StorageError::InvalidInput(format!(
                "stored tool spec map {field} is malformed"
            )));
        };
        match separator {
            ',' => index += 1,
            '}' => return Ok(map),
            _ => {
                return Err(StorageError::InvalidInput(format!(
                    "stored tool spec map {field} is malformed"
                )));
            }
        }
    }
}

fn find_json_string_end(input: &str) -> Result<usize, StorageError> {
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Ok(index),
            _ => {}
        }
    }
    Err(StorageError::InvalidInput(
        "stored tool spec string is malformed".to_string(),
    ))
}

fn skip_json_whitespace(input: &str, mut index: usize) -> usize {
    while let Some(ch) = input[index..].chars().next() {
        if !ch.is_whitespace() {
            break;
        }
        index += ch.len_utf8();
    }
    index
}

fn unescape_json_string(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('"') => output.push('"'),
                Some('\\') => output.push('\\'),
                Some('n') => output.push('\n'),
                Some('r') => output.push('\r'),
                Some('t') => output.push('\t'),
                Some(other) => output.push(other),
                None => {}
            }
        } else {
            output.push(ch);
        }
    }
    output
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
            _ => output.push(ch),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tool(version: &str) -> String {
        format!(
            r#"
schema_version: agentflow.tool.v0
namespace: marker
name: marker_survival_scan
version: {version}
maturity: wrapped
description: "Scan a candidate marker against survival table"
inputs:
  expression_table:
    type: TSV
    required: true
  survival_table:
    type: TSV
    required: true
params:
  gene:
    type: string
    required: true
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#
        )
    }

    fn temp_project_path(test_name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-tool-registry-{test_name}-{}-{}",
            std::process::id(),
            now_unix_seconds()
        ))
    }

    #[test]
    fn parses_v0_simple_yaml_metadata() {
        let spec = ToolSpec::from_simple_yaml(&sample_tool("0.1.0")).unwrap();

        assert_eq!(spec.schema_version, agentflow_schemas::TOOL_SCHEMA_V0);
        assert_eq!(spec.namespace, "marker");
        assert_eq!(spec.name, "marker_survival_scan");
        assert_eq!(spec.version, "0.1.0");
        assert_eq!(spec.maturity, ToolMaturity::Wrapped);
        assert_eq!(spec.tool_ref(), "marker/marker_survival_scan");
        assert_eq!(spec.inputs.len(), 2);
        assert!(spec.inputs["survival_table"].required);
        assert_eq!(spec.outputs["report"].type_name, "Markdown");
        assert_eq!(spec.runtime.backend, "local");
        assert_eq!(spec.runtime.command, ["/bin/echo"]);
    }

    #[test]
    fn parses_output_observer_metadata() {
        let spec = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
namespace: marker
name: marker_survival_scan
version: 0.1.0
maturity: wrapped
description: Scan marker
outputs:
  report:
    type: Markdown
    observer: marker_report
runtime:
  backend: local
  command:
    - /bin/echo
"#,
        )
        .unwrap();

        assert_eq!(
            spec.outputs["report"].observer.as_deref(),
            Some("marker_report")
        );
        assert!(spec.stored_json().contains("\"output_observers\""));
    }

    #[test]
    fn parses_port_validator_metadata() {
        let spec = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
namespace: marker
name: marker_survival_scan
version: 0.1.0
maturity: wrapped
description: Scan marker
inputs:
  expression_table:
    type: TSV
    required: true
    required_columns: sample,TP53
    min_rows: 1
outputs:
  report:
    type: Markdown
    min_rows: 3
runtime:
  backend: local
  command:
    - /bin/echo
"#,
        )
        .unwrap();

        assert_eq!(spec.inputs["expression_table"].min_rows, Some(1));
        assert_eq!(
            spec.inputs["expression_table"].required_columns,
            ["sample".to_string(), "TP53".to_string()]
        );
        assert_eq!(spec.outputs["report"].min_rows, Some(3));
        assert!(spec.stored_json().contains("\"input_required_columns\""));
    }

    #[test]
    fn rejects_unknown_output_observer_adapter() {
        let err = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: bad_observer
version: 0.1.0
maturity: wrapped
description: bad
outputs:
  report:
    type: Markdown
    observer: magic_report
runtime:
  backend: local
  command:
    - /bin/echo
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("unsupported observer adapter"));
    }

    #[test]
    fn rejects_wrong_schema_version() {
        let err = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v99
name: bad
version: 0.1.0
maturity: wrapped
description: bad
outputs:
  report:
    type: 'Markdown,With:Colon'
runtime:
  backend: local
  command:
    - /bin/bad
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("agentflow.tool.v0"));
    }

    #[test]
    fn rejects_tool_without_runtime_command() {
        let err = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: bad
version: 0.1.0
maturity: wrapped
description: bad
outputs:
  report:
    type: Markdown
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("runtime"));
    }

    #[test]
    fn rejects_relative_runtime_executable() {
        let err = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: bad
version: 0.1.0
maturity: wrapped
description: bad
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - bad
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("absolute executable"));
    }

    #[test]
    fn rejects_inline_shell_runtime_command() {
        let err = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: bad
version: 0.1.0
maturity: wrapped
description: bad
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/sh
    - -c
    - echo unsafe
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("inline execution"));
    }

    #[test]
    fn registers_lists_and_inspects_tool() {
        let path = temp_project_path("register-list-inspect");
        let store = ProjectStore::init(&path, Some("Tools")).unwrap();

        let registration = store
            .register_tool(ToolSpec::from_simple_yaml(&sample_tool("0.1.0")).unwrap())
            .unwrap();
        assert_eq!(registration.tool_ref, "marker/marker_survival_scan");
        assert!(!registration.replaced_existing_version);

        let tools = store.list_tools().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool_ref(), "marker/marker_survival_scan");
        assert_eq!(tools[0].latest_version, "0.1.0");

        let inspection = store.inspect_tool("marker/marker_survival_scan").unwrap();
        assert_eq!(inspection.version, "0.1.0");
        assert_eq!(inspection.summary.maturity, "wrapped");
        assert!(inspection.spec_json.contains("marker_survival_scan"));
        assert!(inspection.spec_json.contains("\"output_observers\":{}"));

        let executable = store
            .executable_tool("marker/marker_survival_scan")
            .unwrap();
        assert_eq!(executable.runtime.command, ["/bin/echo"]);
        assert!(executable.inputs["expression_table"].required);
        assert_eq!(executable.outputs["report"].observer, None);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn executable_tool_round_trips_runtime_argv_with_json_punctuation() {
        let path = temp_project_path("runtime-command-roundtrip");
        let store = ProjectStore::init(&path, Some("Tools")).unwrap();

        let spec = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
namespace: marker
name: quoted_command
version: 0.1.0
maturity: wrapped
description: Exercise runtime argv JSON parsing
outputs:
  report:
    type: 'Markdown,With:Colon'
runtime:
  backend: local
  command:
    - /bin/echo
    - -n
    - 'a,b]c'
"#,
        )
        .unwrap();
        store.register_tool(spec).unwrap();

        let executable = store.executable_tool("marker/quoted_command").unwrap();
        assert_eq!(executable.runtime.command, ["/bin/echo", "-n", "a,b]c"]);
        assert_eq!(
            executable.outputs["report"].type_name,
            "Markdown,With:Colon"
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn registering_new_version_updates_latest_version() {
        let path = temp_project_path("version-update");
        let store = ProjectStore::init(&path, Some("Tools")).unwrap();

        store
            .register_tool(ToolSpec::from_simple_yaml(&sample_tool("0.1.0")).unwrap())
            .unwrap();
        store
            .register_tool(ToolSpec::from_simple_yaml(&sample_tool("0.2.0")).unwrap())
            .unwrap();

        let latest = store.inspect_tool("marker/marker_survival_scan").unwrap();
        assert_eq!(latest.version, "0.2.0");

        let old = store
            .inspect_tool("marker/marker_survival_scan@0.1.0")
            .unwrap();
        assert_eq!(old.version, "0.1.0");

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn reregistering_same_version_replaces_version_record() {
        let path = temp_project_path("replace-version");
        let store = ProjectStore::init(&path, Some("Tools")).unwrap();
        let first = store
            .register_tool(ToolSpec::from_simple_yaml(&sample_tool("0.1.0")).unwrap())
            .unwrap();
        let second = store
            .register_tool(ToolSpec::from_simple_yaml(&sample_tool("0.1.0")).unwrap())
            .unwrap();

        assert!(!first.replaced_existing_version);
        assert!(second.replaced_existing_version);
        assert_eq!(store.list_tools().unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(path);
    }
}
