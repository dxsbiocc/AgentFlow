use std::collections::BTreeMap;
use std::path::Path;

use regex::Regex;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::domain::ToolMaturity;

use super::migrations;
use super::project_store::{now_unix_seconds, EventRecord, ProjectStore, StorageError};
use super::yaml;

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
    pub validator_profile: Option<String>,
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
    pub profile: Option<String>,
    pub min_rows: Option<usize>,
    pub required_columns: Vec<String>,
    pub sample_id_column: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolParamSpec {
    pub type_name: String,
    pub required: bool,
    pub enum_values: Option<Vec<String>>,
    pub pattern: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRuntimeSpec {
    pub backend: String,
    pub command: Vec<String>,
    pub timeout_seconds: Option<u64>,
    pub env_name: Option<String>,
    pub env_prefix: Option<String>,
    pub env_file: Option<String>,
    pub runner: Option<String>,
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
        let raw = yaml::parse_yaml::<RawToolSpec>("tool", source_text)?;
        let schema_version =
            required_tool_field(raw.schema_version.clone(), "schema_version", source_text)?;
        if schema_version != agentflow_schemas::TOOL_SCHEMA_V0 {
            return Err(yaml::invalid_input_at_field(
                source_text,
                "schema_version",
                format!(
                    "tool schema_version must be {}",
                    agentflow_schemas::TOOL_SCHEMA_V0
                ),
            ));
        }

        let namespace = raw
            .namespace
            .clone()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_NAMESPACE.to_string());
        let name = required_tool_field(raw.name.clone(), "name", source_text)?;
        let version = required_tool_field(raw.version.clone(), "version", source_text)?;
        let maturity_name = required_tool_field(raw.maturity.clone(), "maturity", source_text)?;
        let description = required_tool_field(raw.description.clone(), "description", source_text)?;
        let maturity = ToolMaturity::parse(&maturity_name).ok_or_else(|| {
            yaml::invalid_input_at_field(
                source_text,
                "maturity",
                format!(
                    "maturity must be one of: verified, wrapped, exploratory; got {maturity_name}"
                ),
            )
        })?;

        validate_ref_part("namespace", &namespace)
            .map_err(|error| yaml::with_field_location(source_text, "namespace", error))?;
        validate_ref_part("name", &name)
            .map_err(|error| yaml::with_field_location(source_text, "name", error))?;
        validate_ref_part("version", &version)
            .map_err(|error| yaml::with_field_location(source_text, "version", error))?;
        let validator_profile = raw
            .validator_profile
            .clone()
            .filter(|value| !value.is_empty());
        if let Some(profile) = validator_profile.as_deref() {
            validate_validator_profile(profile).map_err(|error| {
                yaml::with_field_location(source_text, "validator_profile", error)
            })?;
        }
        let mut executable = raw.into_executable_sections(source_text)?;
        apply_tool_validator_profile(validator_profile.as_deref(), &mut executable.inputs)
            .map_err(|error| yaml::with_field_location(source_text, "validator_profile", error))?;
        apply_input_profiles(&mut executable.inputs)
            .map_err(|error| yaml::with_field_location(source_text, "profile", error))?;
        executable
            .validate()
            .map_err(|error| yaml::with_field_location(source_text, "runtime", error))?;

        Ok(Self {
            schema_version,
            namespace,
            name,
            version,
            maturity,
            description,
            validator_profile,
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
            .filter_map(|(name, port)| port.required.then_some(name.clone()))
            .collect::<Vec<_>>();
        let required_params = self
            .params
            .iter()
            .filter_map(|(name, param)| param.required.then_some(name.clone()))
            .collect::<Vec<_>>();

        serde_json::to_string(&StoredToolSpecJson {
            schema_version: self.schema_version.clone(),
            namespace: self.namespace.clone(),
            name: self.name.clone(),
            version: self.version.clone(),
            maturity: self.maturity.as_str().to_string(),
            description: self.description.clone(),
            validator_profile: self.validator_profile.clone(),
            input_types: port_type_map(&self.inputs),
            required_inputs,
            input_profiles: profile_map(&self.inputs),
            param_types: param_type_map(&self.params),
            required_params,
            param_enum_values: param_enum_values_map(&self.params),
            param_patterns: param_patterns_map(&self.params),
            output_types: port_type_map(&self.outputs),
            output_observers: observer_map(&self.outputs),
            input_min_rows: min_rows_map(&self.inputs),
            input_required_columns: required_columns_map(&self.inputs),
            input_sample_id_columns: sample_id_column_map(&self.inputs),
            output_min_rows: min_rows_map(&self.outputs),
            output_required_columns: required_columns_map(&self.outputs),
            runtime_backend: self.runtime.backend.clone(),
            runtime_command: self.runtime.command.clone(),
            runtime_timeout_seconds: self.runtime.timeout_seconds,
            runtime_env_name: self.runtime.env_name.clone(),
            runtime_env_prefix: self.runtime.env_prefix.clone(),
            runtime_env_file: self.runtime.env_file.clone(),
            runtime_runner: self.runtime.runner.clone(),
            source_format: SIMPLE_YAML_SOURCE_FORMAT.to_string(),
            source_text: self.source_text.clone(),
        })
        .expect("tool spec serializes to stored JSON")
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredToolSpecJson {
    schema_version: String,
    namespace: String,
    name: String,
    version: String,
    maturity: String,
    description: String,
    #[serde(default)]
    validator_profile: Option<String>,
    #[serde(default)]
    input_types: BTreeMap<String, String>,
    #[serde(default)]
    required_inputs: Vec<String>,
    #[serde(default)]
    input_profiles: BTreeMap<String, String>,
    #[serde(default)]
    param_types: BTreeMap<String, String>,
    #[serde(default)]
    required_params: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    param_enum_values: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    param_patterns: BTreeMap<String, String>,
    #[serde(default)]
    output_types: BTreeMap<String, String>,
    #[serde(default)]
    output_observers: BTreeMap<String, String>,
    #[serde(default)]
    input_min_rows: BTreeMap<String, String>,
    #[serde(default)]
    input_required_columns: BTreeMap<String, String>,
    #[serde(default)]
    input_sample_id_columns: BTreeMap<String, String>,
    #[serde(default)]
    output_min_rows: BTreeMap<String, String>,
    #[serde(default)]
    output_required_columns: BTreeMap<String, String>,
    runtime_backend: String,
    #[serde(default)]
    runtime_command: Vec<String>,
    #[serde(default)]
    runtime_timeout_seconds: Option<u64>,
    #[serde(default)]
    runtime_env_name: Option<String>,
    #[serde(default)]
    runtime_env_prefix: Option<String>,
    #[serde(default)]
    runtime_env_file: Option<String>,
    #[serde(default)]
    runtime_runner: Option<String>,
    #[serde(default)]
    source_format: String,
    #[serde(default)]
    source_text: String,
}

#[derive(Deserialize)]
struct RawToolSpec {
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    schema_version: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    namespace: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    name: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    version: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    maturity: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    description: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    validator_profile: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_default_map")]
    inputs: BTreeMap<String, Option<RawToolPortSpec>>,
    #[serde(default, deserialize_with = "yaml::deserialize_default_map")]
    params: BTreeMap<String, Option<RawToolParamSpec>>,
    #[serde(default, deserialize_with = "yaml::deserialize_default_map")]
    outputs: BTreeMap<String, Option<RawToolPortSpec>>,
    #[serde(default)]
    runtime: Option<RawToolRuntimeSpec>,
}

impl RawToolSpec {
    fn into_executable_sections(
        self,
        source_text: &str,
    ) -> Result<ParsedExecutableSections, StorageError> {
        let inputs = self
            .inputs
            .into_iter()
            .map(|(name, raw)| {
                validate_ref_part("tool section item", &name)
                    .map_err(|error| yaml::with_field_location(source_text, &name, error))?;
                let port = raw.unwrap_or_default().into_input_port(source_text)?;
                Ok((name, port))
            })
            .collect::<Result<BTreeMap<_, _>, StorageError>>()?;
        let params = self
            .params
            .into_iter()
            .map(|(name, raw)| {
                validate_ref_part("tool section item", &name)
                    .map_err(|error| yaml::with_field_location(source_text, &name, error))?;
                let param = raw.unwrap_or_default().into_param();
                Ok((name, param))
            })
            .collect::<Result<BTreeMap<_, _>, StorageError>>()?;
        let outputs = self
            .outputs
            .into_iter()
            .map(|(name, raw)| {
                validate_ref_part("tool section item", &name)
                    .map_err(|error| yaml::with_field_location(source_text, &name, error))?;
                let port = raw.unwrap_or_default().into_output_port(source_text)?;
                Ok((name, port))
            })
            .collect::<Result<BTreeMap<_, _>, StorageError>>()?;
        let runtime = self.runtime.unwrap_or_default().into_runtime(source_text)?;

        Ok(ParsedExecutableSections {
            inputs,
            params,
            outputs,
            runtime,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawToolPortSpec {
    #[serde(
        rename = "type",
        default,
        deserialize_with = "yaml::deserialize_optional_scalar_string"
    )]
    type_name: Option<String>,
    #[serde(
        default = "default_true",
        deserialize_with = "yaml::deserialize_bool_like"
    )]
    required: bool,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    observer: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    profile: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    min_rows: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_required_columns")]
    required_columns: Vec<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    sample_id_column: Option<String>,
}

impl Default for RawToolPortSpec {
    fn default() -> Self {
        Self {
            type_name: None,
            required: true,
            observer: None,
            profile: None,
            min_rows: None,
            required_columns: Vec::new(),
            sample_id_column: None,
        }
    }
}

impl RawToolPortSpec {
    fn into_input_port(self, source_text: &str) -> Result<ToolPortSpec, StorageError> {
        if self.observer.is_some() {
            return Err(yaml::invalid_input_at_field(
                source_text,
                "observer",
                "observer is only supported on output ports",
            ));
        }
        let profile = self
            .profile
            .map(|profile| {
                validate_input_profile(&profile)
                    .map(|_| profile)
                    .map_err(|error| yaml::with_field_location(source_text, "profile", error))
            })
            .transpose()?;
        let min_rows = self
            .min_rows
            .map(|value| {
                parse_usize_field("min_rows", &value)
                    .map_err(|error| yaml::with_field_location(source_text, "min_rows", error))
            })
            .transpose()?;
        let required_columns = parse_raw_columns(self.required_columns)
            .map_err(|error| yaml::with_field_location(source_text, "required_columns", error))?;
        let sample_id_column = self
            .sample_id_column
            .map(|value| {
                parse_column_name(&value).map_err(|error| {
                    yaml::with_field_location(source_text, "sample_id_column", error)
                })
            })
            .transpose()?;

        Ok(ToolPortSpec {
            type_name: self.type_name.unwrap_or_default(),
            required: self.required,
            observer: None,
            profile,
            min_rows,
            required_columns,
            sample_id_column,
        })
    }

    fn into_output_port(self, source_text: &str) -> Result<ToolPortSpec, StorageError> {
        if self.profile.is_some() {
            return Err(yaml::invalid_input_at_field(
                source_text,
                "profile",
                "profile is only supported on input ports",
            ));
        }
        if self.sample_id_column.is_some() {
            return Err(yaml::invalid_input_at_field(
                source_text,
                "sample_id_column",
                "sample_id_column is only supported on input ports",
            ));
        }
        let observer = self
            .observer
            .map(|observer| {
                validate_observer_adapter(&observer)
                    .map(|_| observer)
                    .map_err(|error| yaml::with_field_location(source_text, "observer", error))
            })
            .transpose()?;
        let min_rows = self
            .min_rows
            .map(|value| {
                parse_usize_field("min_rows", &value)
                    .map_err(|error| yaml::with_field_location(source_text, "min_rows", error))
            })
            .transpose()?;
        let required_columns = parse_raw_columns(self.required_columns)
            .map_err(|error| yaml::with_field_location(source_text, "required_columns", error))?;

        Ok(ToolPortSpec {
            type_name: self.type_name.unwrap_or_default(),
            required: self.required,
            observer,
            profile: None,
            min_rows,
            required_columns,
            sample_id_column: None,
        })
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawToolParamSpec {
    #[serde(
        rename = "type",
        default,
        deserialize_with = "yaml::deserialize_optional_scalar_string"
    )]
    type_name: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_bool_like")]
    required: bool,
    #[serde(
        rename = "enum",
        default,
        deserialize_with = "yaml::deserialize_string_vec"
    )]
    enum_values: Vec<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    pattern: Option<String>,
}

impl RawToolParamSpec {
    fn into_param(self) -> ToolParamSpec {
        ToolParamSpec {
            type_name: self.type_name.unwrap_or_default(),
            required: self.required,
            enum_values: (!self.enum_values.is_empty()).then_some(self.enum_values),
            pattern: self.pattern,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawToolRuntimeSpec {
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    backend: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_string_vec")]
    command: Vec<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    timeout_seconds: Option<String>,
    #[serde(
        default,
        deserialize_with = "yaml::deserialize_optional_present_scalar_string"
    )]
    env_name: Option<String>,
    #[serde(
        default,
        deserialize_with = "yaml::deserialize_optional_present_scalar_string"
    )]
    env_prefix: Option<String>,
    #[serde(
        default,
        deserialize_with = "yaml::deserialize_optional_present_scalar_string"
    )]
    env_file: Option<String>,
    #[serde(
        default,
        deserialize_with = "yaml::deserialize_optional_present_scalar_string"
    )]
    runner: Option<String>,
}

impl RawToolRuntimeSpec {
    fn into_runtime(self, source_text: &str) -> Result<ToolRuntimeSpec, StorageError> {
        Ok(ToolRuntimeSpec {
            backend: self.backend.unwrap_or_default(),
            command: self.command,
            timeout_seconds: self
                .timeout_seconds
                .map(|value| {
                    parse_u64_field("runtime.timeout_seconds", &value).map_err(|error| {
                        yaml::with_field_location(source_text, "timeout_seconds", error)
                    })
                })
                .transpose()?,
            env_name: self
                .env_name
                .map(|value| {
                    parse_runtime_string("runtime.env_name", &value)
                        .map_err(|error| yaml::with_field_location(source_text, "env_name", error))
                })
                .transpose()?,
            env_prefix: self
                .env_prefix
                .map(|value| {
                    parse_runtime_string("runtime.env_prefix", &value).map_err(|error| {
                        yaml::with_field_location(source_text, "env_prefix", error)
                    })
                })
                .transpose()?,
            env_file: self
                .env_file
                .map(|value| {
                    parse_runtime_string("runtime.env_file", &value)
                        .map_err(|error| yaml::with_field_location(source_text, "env_file", error))
                })
                .transpose()?,
            runner: self
                .runner
                .map(|value| {
                    parse_runtime_string("runtime.runner", &value)
                        .map_err(|error| yaml::with_field_location(source_text, "runner", error))
                })
                .transpose()?,
        })
    }
}

fn required_tool_field(
    value: Option<String>,
    field_name: &str,
    source_text: &str,
) -> Result<String, StorageError> {
    value.filter(|value| !value.is_empty()).ok_or_else(|| {
        yaml::invalid_input_at_field(
            source_text,
            field_name,
            format!("tool spec is missing required field {field_name}"),
        )
    })
}

fn parse_raw_columns(columns: Vec<String>) -> Result<Vec<String>, StorageError> {
    columns
        .into_iter()
        .map(|column| parse_column_name_with_label("required_columns", &column))
        .collect()
}

fn default_true() -> bool {
    true
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
        validate_runtime_backend(&self.runtime)?;
        if self.runtime.command.is_empty() {
            return Err(StorageError::InvalidInput(
                "tool spec runtime.command must contain at least one argv entry".to_string(),
            ));
        }
        if self.runtime.backend == "local" && !Path::new(&self.runtime.command[0]).is_absolute() {
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
            if let Some(pattern) = &param.pattern {
                Regex::new(pattern).map_err(|error| {
                    StorageError::InvalidInput(format!("param {name} pattern is invalid: {error}"))
                })?;
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
        serde_json::to_string(&ToolInspectionJson {
            schema_version: agentflow_schemas::TOOL_INSPECTION_JSON_SCHEMA_V0.to_string(),
            tool: ToolInspectionToolJson {
                tool_ref: self.summary.tool_ref(),
                namespace: self.summary.namespace.clone(),
                name: self.summary.name.clone(),
                latest_version: self.summary.latest_version.clone(),
                maturity: self.summary.maturity.clone(),
            },
            version: ToolInspectionVersionJson {
                id: self.version_id.clone(),
                version: self.version.clone(),
                schema_version: self.schema_version.clone(),
                spec_hash: self.spec_hash.clone(),
                created_at: self.created_at,
                spec: stored_tool_spec_from_json(&self.spec_json)
                    .expect("stored tool spec JSON is valid"),
            },
        })
        .expect("tool inspection serializes to JSON")
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ToolInspectionJson {
    schema_version: String,
    tool: ToolInspectionToolJson,
    version: ToolInspectionVersionJson,
}

#[derive(Debug, Serialize, Deserialize)]
struct ToolInspectionToolJson {
    #[serde(rename = "ref")]
    tool_ref: String,
    namespace: String,
    name: String,
    latest_version: String,
    maturity: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ToolInspectionVersionJson {
    id: String,
    version: String,
    schema_version: String,
    spec_hash: String,
    created_at: i64,
    spec: StoredToolSpecJson,
}

#[derive(Debug, Serialize, Deserialize)]
struct ToolRegisteredPayload {
    tool_ref: String,
    version: String,
    spec_hash: String,
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
            payload_json: serde_json::to_string(&ToolRegisteredPayload {
                tool_ref: spec.tool_ref(),
                version: spec.version.clone(),
                spec_hash: spec_hash.clone(),
            })
            .expect("tool registered payload serializes to JSON"),
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
        columns.push(parse_column_name_with_label("required_columns", column)?);
    }
    if columns.is_empty() {
        return Err(StorageError::InvalidInput(
            "required_columns must list at least one column".to_string(),
        ));
    }
    Ok(columns)
}

fn parse_column_name(value: &str) -> Result<String, StorageError> {
    parse_column_name_with_label("sample_id_column", value)
}

fn parse_column_name_with_label(label: &str, value: &str) -> Result<String, StorageError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(StorageError::InvalidInput(format!(
            "{label} must not contain empty column names"
        )));
    }
    if value.contains('\n') || value.contains('\0') {
        return Err(StorageError::InvalidInput(format!(
            "{label} entries must be single-line text"
        )));
    }
    Ok(value.to_string())
}

fn apply_tool_validator_profile(
    profile: Option<&str>,
    inputs: &mut BTreeMap<String, ToolPortSpec>,
) -> Result<(), StorageError> {
    match profile {
        None => Ok(()),
        Some("paired_expression_survival_v0") => {
            let expression = inputs.get_mut("expression_table").ok_or_else(|| {
                StorageError::InvalidInput(
                    "validator_profile paired_expression_survival_v0 requires input expression_table"
                        .to_string(),
                )
            })?;
            ensure_profile(
                "expression_table",
                expression,
                "expression_table_v0",
                "paired_expression_survival_v0",
            )?;
            let survival = inputs.get_mut("survival_table").ok_or_else(|| {
                StorageError::InvalidInput(
                    "validator_profile paired_expression_survival_v0 requires input survival_table"
                        .to_string(),
                )
            })?;
            ensure_profile(
                "survival_table",
                survival,
                "survival_table_v0",
                "paired_expression_survival_v0",
            )?;
            Ok(())
        }
        Some(profile) => Err(StorageError::InvalidInput(format!(
            "unsupported validator_profile {profile}"
        ))),
    }
}

fn ensure_profile(
    input_name: &str,
    port: &mut ToolPortSpec,
    expected_profile: &str,
    validator_profile: &str,
) -> Result<(), StorageError> {
    match port.profile.as_deref() {
        Some(profile) if profile == expected_profile => Ok(()),
        Some(profile) => Err(StorageError::InvalidInput(format!(
            "validator_profile {validator_profile} requires input {input_name} profile {expected_profile}, got {profile}"
        ))),
        None => {
            port.profile = Some(expected_profile.to_string());
            Ok(())
        }
    }
}

fn apply_input_profiles(inputs: &mut BTreeMap<String, ToolPortSpec>) -> Result<(), StorageError> {
    for port in inputs.values_mut() {
        let Some(profile) = port.profile.clone() else {
            continue;
        };
        let defaults = input_profile_defaults(&profile)?;
        if port.type_name.trim().is_empty() {
            port.type_name = defaults.type_name.to_string();
        }
        if port.min_rows.is_none() {
            port.min_rows = Some(defaults.min_rows);
        }
        if port.required_columns.is_empty() {
            port.required_columns = defaults
                .required_columns
                .iter()
                .map(|column| (*column).to_string())
                .collect();
        }
        if port.sample_id_column.is_none() {
            port.sample_id_column = Some(defaults.sample_id_column.to_string());
        }
    }
    Ok(())
}

struct InputProfileDefaults {
    type_name: &'static str,
    min_rows: usize,
    required_columns: &'static [&'static str],
    sample_id_column: &'static str,
}

fn input_profile_defaults(profile: &str) -> Result<InputProfileDefaults, StorageError> {
    match profile {
        "expression_table_v0" => Ok(InputProfileDefaults {
            type_name: "TSV",
            min_rows: 1,
            required_columns: &["sample"],
            sample_id_column: "sample",
        }),
        "survival_table_v0" => Ok(InputProfileDefaults {
            type_name: "TSV",
            min_rows: 1,
            required_columns: &["sample", "time", "status"],
            sample_id_column: "sample",
        }),
        other => Err(StorageError::InvalidInput(format!(
            "unsupported input profile {other}"
        ))),
    }
}

fn validate_validator_profile(profile: &str) -> Result<(), StorageError> {
    validate_ref_part("validator_profile", profile)?;
    if profile == "paired_expression_survival_v0" {
        Ok(())
    } else {
        Err(StorageError::InvalidInput(format!(
            "unsupported validator_profile {profile}"
        )))
    }
}

fn validate_input_profile(profile: &str) -> Result<(), StorageError> {
    validate_ref_part("input profile", profile)?;
    input_profile_defaults(profile).map(|_| ())
}

fn validate_ports(label: &str, ports: &BTreeMap<String, ToolPortSpec>) -> Result<(), StorageError> {
    for (name, port) in ports {
        validate_ref_part(&format!("{label} name"), name)?;
        if port.type_name.trim().is_empty() {
            return Err(StorageError::InvalidInput(format!(
                "{label} {name} must declare type"
            )));
        }
        if let Some(profile) = port.profile.as_deref() {
            if label != "input" {
                return Err(StorageError::InvalidInput(format!(
                    "{label} {name} must not declare profile"
                )));
            }
            validate_input_profile(profile)?;
        }
        if let Some(observer) = port.observer.as_deref() {
            if label != "output" {
                return Err(StorageError::InvalidInput(format!(
                    "{label} {name} must not declare observer"
                )));
            }
            validate_observer_adapter(observer)?;
        }
        if label != "input" && port.sample_id_column.is_some() {
            return Err(StorageError::InvalidInput(format!(
                "{label} {name} must not declare sample_id_column"
            )));
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

fn stored_sample_id_column(
    map: &BTreeMap<String, String>,
    name: &str,
) -> Result<Option<String>, StorageError> {
    map.get(name)
        .map(|value| parse_column_name(value))
        .transpose()
}

fn parse_u64_field(field_name: &str, value: &str) -> Result<u64, StorageError> {
    let parsed = value.parse::<u64>().map_err(|_| {
        StorageError::InvalidInput(format!("{field_name} must be a positive integer"))
    })?;
    if parsed == 0 {
        return Err(StorageError::InvalidInput(format!(
            "{field_name} must be greater than zero"
        )));
    }
    Ok(parsed)
}

fn parse_runtime_string(field_name: &str, value: &str) -> Result<String, StorageError> {
    let value = normalize_scalar(value.trim());
    if value.trim().is_empty() || value.contains('\n') || value.contains('\0') {
        return Err(StorageError::InvalidInput(format!(
            "{field_name} must be non-empty single-line text"
        )));
    }
    Ok(value)
}

fn validate_runtime_backend(runtime: &ToolRuntimeSpec) -> Result<(), StorageError> {
    match runtime.backend.as_str() {
        "local" => {
            if runtime.env_name.is_some()
                || runtime.env_prefix.is_some()
                || runtime.env_file.is_some()
                || runtime.runner.is_some()
            {
                return Err(StorageError::InvalidInput(
                    "local runtime must not declare env_name, env_prefix, env_file, or runner"
                        .to_string(),
                ));
            }
            Ok(())
        }
        "conda" | "micromamba" => {
            match (runtime.env_name.as_deref(), runtime.env_prefix.as_deref()) {
                (Some(_), Some(_)) => {
                    return Err(StorageError::InvalidInput(
                        "environment runtime must declare only one of env_name or env_prefix"
                            .to_string(),
                    ));
                }
                (None, None) => {
                    return Err(StorageError::InvalidInput(
                        "environment runtime must declare env_name or env_prefix".to_string(),
                    ));
                }
                (Some(env_name), None) => validate_ref_part("runtime.env_name", env_name)?,
                (None, Some(env_prefix)) => {
                    validate_runtime_path("runtime.env_prefix", env_prefix)?
                }
            }
            if let Some(env_file) = runtime.env_file.as_deref() {
                validate_runtime_path("runtime.env_file", env_file)?;
            }
            let runner = runtime.runner.as_deref().ok_or_else(|| {
                StorageError::InvalidInput(
                    "environment runtime must declare absolute runner path".to_string(),
                )
            })?;
            validate_runtime_path("runtime.runner", runner)?;
            if !Path::new(runner).is_absolute() {
                return Err(StorageError::InvalidInput(
                    "runtime.runner must be an absolute executable path".to_string(),
                ));
            }
            Ok(())
        }
        other => Err(StorageError::InvalidInput(format!(
            "unsupported runtime.backend {other}; supported backends are local, conda, micromamba"
        ))),
    }
}

fn validate_runtime_path(field_name: &str, value: &str) -> Result<(), StorageError> {
    if value.trim().is_empty() || value.contains('\n') || value.contains('\0') {
        return Err(StorageError::InvalidInput(format!(
            "{field_name} must be non-empty single-line text"
        )));
    }
    Ok(())
}

fn executable_from_stored_json(
    tool_ref: &str,
    version: &str,
    spec_json: &str,
) -> Result<ExecutableToolSpec, StorageError> {
    let stored = stored_tool_spec_from_json(spec_json)?;
    let required_inputs = stored
        .required_inputs
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let required_params = stored
        .required_params
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let runtime = ToolRuntimeSpec {
        backend: stored.runtime_backend,
        command: stored.runtime_command,
        timeout_seconds: stored.runtime_timeout_seconds,
        env_name: stored.runtime_env_name,
        env_prefix: stored.runtime_env_prefix,
        env_file: stored.runtime_env_file,
        runner: stored.runtime_runner,
    };

    let inputs = stored
        .input_types
        .into_iter()
        .map(|(name, type_name)| {
            let required = required_inputs.contains(&name);
            let min_rows = stored_min_rows(&stored.input_min_rows, &name)?;
            let required_columns = stored_columns(&stored.input_required_columns, &name)?;
            let sample_id_column = stored_sample_id_column(&stored.input_sample_id_columns, &name)?;
            let profile = stored.input_profiles.get(&name).cloned();
            Ok((
                name,
                ToolPortSpec {
                    type_name,
                    required,
                    observer: None,
                    profile,
                    min_rows,
                    required_columns,
                    sample_id_column,
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>, StorageError>>()?;
    let params = stored
        .param_types
        .into_iter()
        .map(|(name, type_name)| {
            let required = required_params.contains(&name);
            let enum_values = stored.param_enum_values.get(&name).cloned();
            let pattern = stored.param_patterns.get(&name).cloned();
            (
                name,
                ToolParamSpec {
                    type_name,
                    required,
                    enum_values,
                    pattern,
                },
            )
        })
        .collect();
    let outputs = stored
        .output_types
        .into_iter()
        .map(|(name, type_name)| {
            let observer = stored.output_observers.get(&name).cloned();
            let min_rows = stored_min_rows(&stored.output_min_rows, &name)?;
            let required_columns = stored_columns(&stored.output_required_columns, &name)?;
            Ok((
                name,
                ToolPortSpec {
                    type_name,
                    required: true,
                    observer,
                    profile: None,
                    min_rows,
                    required_columns,
                    sample_id_column: None,
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

fn stored_tool_spec_from_json(spec_json: &str) -> Result<StoredToolSpecJson, StorageError> {
    serde_json::from_str(spec_json).map_err(|err| {
        StorageError::InvalidInput(format!("stored tool spec JSON is invalid: {err}"))
    })
}

pub(crate) fn validate_param_value(spec: &ToolParamSpec, value: &str) -> Result<(), String> {
    match spec.type_name.trim() {
        "int" => {
            value
                .parse::<i64>()
                .map_err(|_| format!("value {value:?} must be an int"))?;
        }
        "float" => {
            value
                .parse::<f64>()
                .map_err(|_| format!("value {value:?} must be a float"))?;
        }
        "bool" => {
            value
                .parse::<bool>()
                .map_err(|_| format!("value {value:?} must be a bool"))?;
        }
        "string" => {}
        _ => {}
    }

    if let Some(enum_values) = &spec.enum_values {
        if !enum_values.iter().any(|allowed| allowed == value) {
            return Err(format!(
                "value {value:?} must be one of: {}",
                enum_values.join(", ")
            ));
        }
    }

    if let Some(pattern) = &spec.pattern {
        let regex =
            Regex::new(pattern).map_err(|error| format!("param pattern is invalid: {error}"))?;
        if !regex.is_match(value) {
            return Err(format!("value {value:?} must match pattern {pattern:?}"));
        }
    }

    Ok(())
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

fn port_type_map(map: &BTreeMap<String, ToolPortSpec>) -> BTreeMap<String, String> {
    map.iter()
        .map(|(name, port)| (name.clone(), port.type_name.clone()))
        .collect()
}

fn observer_map(map: &BTreeMap<String, ToolPortSpec>) -> BTreeMap<String, String> {
    map.iter()
        .filter_map(|(name, port)| {
            port.observer
                .as_ref()
                .map(|observer| (name.clone(), observer.clone()))
        })
        .collect()
}

fn profile_map(map: &BTreeMap<String, ToolPortSpec>) -> BTreeMap<String, String> {
    map.iter()
        .filter_map(|(name, port)| {
            port.profile
                .as_ref()
                .map(|profile| (name.clone(), profile.clone()))
        })
        .collect()
}

fn min_rows_map(map: &BTreeMap<String, ToolPortSpec>) -> BTreeMap<String, String> {
    map.iter()
        .filter_map(|(name, port)| {
            port.min_rows
                .map(|min_rows| (name.clone(), min_rows.to_string()))
        })
        .collect()
}

fn required_columns_map(map: &BTreeMap<String, ToolPortSpec>) -> BTreeMap<String, String> {
    map.iter()
        .filter(|(_, port)| !port.required_columns.is_empty())
        .map(|(name, port)| (name.clone(), port.required_columns.join(",")))
        .collect()
}

fn sample_id_column_map(map: &BTreeMap<String, ToolPortSpec>) -> BTreeMap<String, String> {
    map.iter()
        .filter_map(|(name, port)| {
            port.sample_id_column
                .as_ref()
                .map(|column| (name.clone(), column.clone()))
        })
        .collect()
}

fn param_type_map(map: &BTreeMap<String, ToolParamSpec>) -> BTreeMap<String, String> {
    map.iter()
        .map(|(name, param)| (name.clone(), param.type_name.clone()))
        .collect()
}

fn param_enum_values_map(map: &BTreeMap<String, ToolParamSpec>) -> BTreeMap<String, Vec<String>> {
    map.iter()
        .filter_map(|(name, param)| {
            param
                .enum_values
                .as_ref()
                .map(|values| (name.clone(), values.clone()))
        })
        .collect()
}

fn param_patterns_map(map: &BTreeMap<String, ToolParamSpec>) -> BTreeMap<String, String> {
    map.iter()
        .filter_map(|(name, param)| {
            param
                .pattern
                .as_ref()
                .map(|pattern| (name.clone(), pattern.clone()))
        })
        .collect()
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
    fn parses_inline_tool_yaml_equivalent_to_block_form() {
        let block = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
namespace: marker
name: inline_equivalent
version: 0.1.0
maturity: wrapped
description: Inline-equivalent tool
inputs:
  expression_table:
    type: TSV
    required: true
    required_columns: sample,TP53
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
    - /bin/sh
    - x.sh
"#,
        )
        .unwrap();
        let mut inline = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
namespace: marker
name: inline_equivalent
version: 0.1.0
maturity: wrapped
description: Inline-equivalent tool
inputs: {expression_table: {type: TSV, required: true, required_columns: [sample, TP53]}}
params: {gene: {type: string, required: true}}
outputs: {report: {type: Markdown}}
runtime:
  backend: local
  command: [/bin/sh, x.sh]
"#,
        )
        .unwrap();

        inline.source_text = block.source_text.clone();
        assert_eq!(inline, block);
    }

    #[test]
    fn yaml_parse_errors_are_invalid_input_with_location() {
        let err = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: broken
runtime: [unterminated
"#,
        )
        .unwrap_err();

        assert!(matches!(err, StorageError::InvalidInput(_)));
        let message = err.to_string();
        assert!(message.contains("line"), "{message}");
        assert!(message.contains("column"), "{message}");
    }

    #[test]
    fn yaml_validation_errors_are_invalid_input_with_location() {
        let err = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: bad_maturity
version: 0.1.0
maturity: magic
description: Bad maturity
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#,
        )
        .unwrap_err();

        assert!(matches!(err, StorageError::InvalidInput(_)));
        let message = err.to_string();
        assert!(message.contains("maturity"), "{message}");
        assert!(message.contains("line"), "{message}");
        assert!(message.contains("column"), "{message}");
    }

    #[test]
    fn parses_runtime_timeout_seconds_metadata() {
        let spec = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: timeout_tool
version: 0.1.0
maturity: wrapped
description: Tool with a local runtime timeout
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  timeout_seconds: 5
  command:
    - /bin/echo
"#,
        )
        .unwrap();

        assert_eq!(spec.runtime.timeout_seconds, Some(5));
        assert!(spec.stored_json().contains("\"runtime_timeout_seconds\":5"));

        let path = temp_project_path("runtime-timeout-roundtrip");
        let store = ProjectStore::init(&path, Some("Tools")).unwrap();
        store.register_tool(spec).unwrap();
        let executable = store.executable_tool("local/timeout_tool").unwrap();
        assert_eq!(executable.runtime.timeout_seconds, Some(5));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn rejects_invalid_runtime_timeout_seconds() {
        let zero = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: bad_timeout
version: 0.1.0
maturity: wrapped
description: zero timeout
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  timeout_seconds: 0
  command:
    - /bin/echo
"#,
        )
        .unwrap_err();
        assert!(zero.to_string().contains("greater than zero"));

        let non_numeric = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: bad_timeout
version: 0.1.0
maturity: wrapped
description: non numeric timeout
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  timeout_seconds: soon
  command:
    - /bin/echo
"#,
        )
        .unwrap_err();
        assert!(non_numeric.to_string().contains("positive integer"));
    }

    #[test]
    fn parses_conda_runtime_metadata() {
        let spec = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: conda_tool
version: 0.1.0
maturity: wrapped
description: Tool with a conda runtime wrapper
outputs:
  report:
    type: Markdown
runtime:
  backend: conda
  runner: /opt/conda/bin/conda
  env_name: af-test
  env_file: envs/analysis.yml
  timeout_seconds: 5
  command:
    - python
    - tools/run.py
"#,
        )
        .unwrap();

        assert_eq!(spec.runtime.backend, "conda");
        assert_eq!(spec.runtime.runner.as_deref(), Some("/opt/conda/bin/conda"));
        assert_eq!(spec.runtime.env_name.as_deref(), Some("af-test"));
        assert_eq!(spec.runtime.env_file.as_deref(), Some("envs/analysis.yml"));
        assert_eq!(spec.runtime.command, ["python", "tools/run.py"]);
        assert!(spec.stored_json().contains("\"runtime_env_name\""));

        let path = temp_project_path("conda-runtime-roundtrip");
        let store = ProjectStore::init(&path, Some("Tools")).unwrap();
        store.register_tool(spec).unwrap();
        let executable = store.executable_tool("local/conda_tool").unwrap();
        assert_eq!(executable.runtime.backend, "conda");
        assert_eq!(executable.runtime.env_name.as_deref(), Some("af-test"));
        assert_eq!(
            executable.runtime.runner.as_deref(),
            Some("/opt/conda/bin/conda")
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn rejects_environment_runtime_without_runner_or_env_selector() {
        let missing_env = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: bad_conda
version: 0.1.0
maturity: wrapped
description: bad
outputs:
  report:
    type: Markdown
runtime:
  backend: conda
  runner: /opt/conda/bin/conda
  command:
    - python
"#,
        )
        .unwrap_err();
        assert!(missing_env.to_string().contains("env_name or env_prefix"));

        let missing_runner = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: bad_conda
version: 0.1.0
maturity: wrapped
description: bad
outputs:
  report:
    type: Markdown
runtime:
  backend: micromamba
  env_name: af-test
  command:
    - python
"#,
        )
        .unwrap_err();
        assert!(missing_runner.to_string().contains("runner"));
    }

    #[test]
    fn rejects_local_runtime_environment_fields() {
        let err = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: bad_local_env
version: 0.1.0
maturity: wrapped
description: bad
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  env_name: af-test
  command:
    - /bin/echo
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("local runtime must not declare"));
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
    sample_id_column: sample
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
        assert_eq!(
            spec.inputs["expression_table"].sample_id_column.as_deref(),
            Some("sample")
        );
        assert_eq!(spec.outputs["report"].min_rows, Some(3));
        assert!(spec.stored_json().contains("\"input_required_columns\""));
        assert!(spec.stored_json().contains("\"input_sample_id_columns\""));
    }

    #[test]
    fn input_profile_expands_defaults_without_repeating_fields() {
        let spec = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: profiled_expression
version: 0.1.0
maturity: wrapped
description: Profile-backed expression table
inputs:
  expression_table:
    profile: expression_table_v0
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#,
        )
        .unwrap();

        let input = &spec.inputs["expression_table"];
        assert_eq!(input.profile.as_deref(), Some("expression_table_v0"));
        assert_eq!(input.type_name, "TSV");
        assert_eq!(input.min_rows, Some(1));
        assert_eq!(input.required_columns, ["sample".to_string()]);
        assert_eq!(input.sample_id_column.as_deref(), Some("sample"));
        assert!(spec.stored_json().contains("\"input_profiles\""));

        let path = temp_project_path("input-profile-roundtrip");
        let store = ProjectStore::init(&path, Some("Tools")).unwrap();
        store.register_tool(spec).unwrap();
        let executable = store.executable_tool("local/profiled_expression").unwrap();
        let input = &executable.inputs["expression_table"];
        assert_eq!(input.profile.as_deref(), Some("expression_table_v0"));
        assert_eq!(input.type_name, "TSV");
        assert_eq!(input.min_rows, Some(1));
        assert_eq!(input.required_columns, ["sample".to_string()]);
        assert_eq!(input.sample_id_column.as_deref(), Some("sample"));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn explicit_profile_fields_override_defaults() {
        let spec = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: profiled_expression
version: 0.1.0
maturity: wrapped
description: Profile-backed expression table with explicit overrides
inputs:
  expression_table:
    profile: expression_table_v0
    required_columns: sample,TP53
    min_rows: 2
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#,
        )
        .unwrap();

        let input = &spec.inputs["expression_table"];
        assert_eq!(input.profile.as_deref(), Some("expression_table_v0"));
        assert_eq!(input.type_name, "TSV");
        assert_eq!(input.min_rows, Some(2));
        assert_eq!(
            input.required_columns,
            ["sample".to_string(), "TP53".to_string()]
        );
        assert_eq!(input.sample_id_column.as_deref(), Some("sample"));
    }

    #[test]
    fn tool_validator_profile_applies_paired_expression_survival_defaults() {
        let spec = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: paired_scan
version: 0.1.0
maturity: wrapped
description: Paired expression and survival analysis
validator_profile: paired_expression_survival_v0
inputs:
  expression_table:
  survival_table:
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#,
        )
        .unwrap();

        assert_eq!(
            spec.validator_profile.as_deref(),
            Some("paired_expression_survival_v0")
        );
        let expression = &spec.inputs["expression_table"];
        assert_eq!(expression.profile.as_deref(), Some("expression_table_v0"));
        assert_eq!(expression.type_name, "TSV");
        assert_eq!(expression.required_columns, ["sample".to_string()]);
        assert_eq!(expression.sample_id_column.as_deref(), Some("sample"));

        let survival = &spec.inputs["survival_table"];
        assert_eq!(survival.profile.as_deref(), Some("survival_table_v0"));
        assert_eq!(survival.type_name, "TSV");
        assert_eq!(
            survival.required_columns,
            [
                "sample".to_string(),
                "time".to_string(),
                "status".to_string()
            ]
        );
        assert_eq!(survival.sample_id_column.as_deref(), Some("sample"));
    }

    #[test]
    fn rejects_unknown_input_profile() {
        let err = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: bad_profile
version: 0.1.0
maturity: wrapped
description: bad
inputs:
  table:
    profile: unknown_table_v0
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("unsupported input profile"));
    }

    #[test]
    fn rejects_unknown_validator_profile() {
        let err = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: bad_validator_profile
version: 0.1.0
maturity: wrapped
description: bad
validator_profile: magic_validator_v0
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("unsupported validator_profile"));
    }

    #[test]
    fn rejects_conflicting_input_profile_for_tool_validator_profile() {
        let err = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: conflicting_validator_profile
version: 0.1.0
maturity: wrapped
description: bad
validator_profile: paired_expression_survival_v0
inputs:
  expression_table:
    profile: survival_table_v0
  survival_table:
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#,
        )
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("requires input expression_table profile"));
    }

    #[test]
    fn rejects_sample_id_column_on_outputs() {
        let err = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
name: bad_sample_id_output
version: 0.1.0
maturity: wrapped
description: bad
outputs:
  report:
    type: Markdown
    sample_id_column: sample
runtime:
  backend: local
  command:
    - /bin/echo
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("sample_id_column"));
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

    #[test]
    fn stored_tool_spec_hashes_stay_byte_identical_for_examples() {
        let examples_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/tools");
        let marker_json =
            example_tool_spec(&examples_root, "marker_survival_scan.tool.yaml").stored_json();
        let tcga_json =
            example_tool_spec(&examples_root, "tcga_survival_assoc.tool.yaml").stored_json();

        assert_eq!(migrations::checksum(&marker_json), "7368596c7e71f739");
        assert_eq!(migrations::checksum(&tcga_json), "ba5f8753e95845de");
        let marker_payload: StoredToolSpecJson = serde_json::from_str(&marker_json).unwrap();
        assert_eq!(marker_payload.name, "marker_survival_scan");
    }

    #[test]
    fn parses_param_value_constraints_and_executable_tool_preserves_them() {
        let spec = ToolSpec::from_simple_yaml(
            r#"
schema_version: agentflow.tool.v0
namespace: marker
name: constrained_scan
version: 0.1.0
maturity: wrapped
description: Scan with constrained params
params:
  mode:
    type: string
    required: true
    enum: [fast, careful]
  gene:
    type: string
    required: true
    pattern: "^[A-Z0-9-]+$"
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#,
        )
        .unwrap();

        assert_eq!(
            spec.params
                .get("mode")
                .unwrap()
                .enum_values
                .as_ref()
                .unwrap(),
            &vec!["fast".to_string(), "careful".to_string()]
        );
        assert_eq!(
            spec.params.get("gene").unwrap().pattern.as_deref(),
            Some("^[A-Z0-9-]+$")
        );

        let path = temp_project_path("param-constraints");
        let store = ProjectStore::init(&path, Some("Tools")).unwrap();
        store.register_tool(spec).unwrap();
        let executable = store.executable_tool("marker/constrained_scan").unwrap();
        assert_eq!(
            executable
                .params
                .get("mode")
                .unwrap()
                .enum_values
                .as_ref()
                .unwrap(),
            &vec!["fast".to_string(), "careful".to_string()]
        );
        assert_eq!(
            executable.params.get("gene").unwrap().pattern.as_deref(),
            Some("^[A-Z0-9-]+$")
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn validate_param_value_checks_type_enum_and_pattern() {
        let int_param = ToolParamSpec {
            type_name: "int".to_string(),
            required: true,
            enum_values: None,
            pattern: None,
        };
        assert!(validate_param_value(&int_param, "42").is_ok());
        assert!(validate_param_value(&int_param, "4.2")
            .unwrap_err()
            .contains("must be an int"));

        let enum_param = ToolParamSpec {
            type_name: "string".to_string(),
            required: true,
            enum_values: Some(vec!["fast".to_string(), "careful".to_string()]),
            pattern: None,
        };
        assert!(validate_param_value(&enum_param, "fast").is_ok());
        assert!(validate_param_value(&enum_param, "slow")
            .unwrap_err()
            .contains("must be one of"));

        let pattern_param = ToolParamSpec {
            type_name: "string".to_string(),
            required: true,
            enum_values: None,
            pattern: Some("^[A-Z0-9-]+$".to_string()),
        };
        assert!(validate_param_value(&pattern_param, "TP53").is_ok());
        assert!(validate_param_value(&pattern_param, "TP53!")
            .unwrap_err()
            .contains("must match pattern"));
    }

    fn example_tool_spec(examples_root: &std::path::Path, file_name: &str) -> ToolSpec {
        // Hash the spec exactly as parsed from YAML. Production registration keeps
        // the command path verbatim (it never canonicalizes), so we must not bake a
        // machine-specific absolute path into the golden hash -- doing so made this
        // test pass only on the machine that generated the constant and fail on CI.
        let spec_path = examples_root.join(file_name);
        let source = std::fs::read_to_string(&spec_path).unwrap();
        ToolSpec::from_simple_yaml(&source).unwrap()
    }

    #[test]
    fn tool_inspection_json_and_event_payload_are_serde_readable() {
        let inspection = ToolInspection {
            summary: ToolSummary {
                id: "tool:marker/scan".to_string(),
                namespace: "marker".to_string(),
                name: "scan".to_string(),
                latest_version: "0.1.0".to_string(),
                maturity: "wrapped".to_string(),
            },
            version_id: "tool_version:marker/scan@0.1.0".to_string(),
            version: "0.1.0".to_string(),
            schema_version: agentflow_schemas::TOOL_SCHEMA_V0.to_string(),
            spec_hash: "abc123".to_string(),
            created_at: 7,
            spec_json: "{\"schema_version\":\"agentflow.tool.v0\",\"namespace\":\"marker\",\"name\":\"scan\",\"version\":\"0.1.0\",\"maturity\":\"wrapped\",\"description\":\"Scan\",\"validator_profile\":null,\"input_types\":{},\"required_inputs\":[],\"input_profiles\":{},\"param_types\":{},\"required_params\":[],\"output_types\":{},\"output_observers\":{},\"input_min_rows\":{},\"input_required_columns\":{},\"input_sample_id_columns\":{},\"output_min_rows\":{},\"output_required_columns\":{},\"runtime_backend\":\"local\",\"runtime_command\":[\"/bin/echo\"],\"runtime_timeout_seconds\":null,\"runtime_env_name\":null,\"runtime_env_prefix\":null,\"runtime_env_file\":null,\"runtime_runner\":null,\"source_format\":\"agentflow.tool.v0.simple_yaml\",\"source_text\":\"source\"}".to_string(),
        };

        assert_eq!(
            inspection.to_json(),
            "{\"schema_version\":\"agentflow.tool_inspection.v0\",\"tool\":{\"ref\":\"marker/scan\",\"namespace\":\"marker\",\"name\":\"scan\",\"latest_version\":\"0.1.0\",\"maturity\":\"wrapped\"},\"version\":{\"id\":\"tool_version:marker/scan@0.1.0\",\"version\":\"0.1.0\",\"schema_version\":\"agentflow.tool.v0\",\"spec_hash\":\"abc123\",\"created_at\":7,\"spec\":{\"schema_version\":\"agentflow.tool.v0\",\"namespace\":\"marker\",\"name\":\"scan\",\"version\":\"0.1.0\",\"maturity\":\"wrapped\",\"description\":\"Scan\",\"validator_profile\":null,\"input_types\":{},\"required_inputs\":[],\"input_profiles\":{},\"param_types\":{},\"required_params\":[],\"output_types\":{},\"output_observers\":{},\"input_min_rows\":{},\"input_required_columns\":{},\"input_sample_id_columns\":{},\"output_min_rows\":{},\"output_required_columns\":{},\"runtime_backend\":\"local\",\"runtime_command\":[\"/bin/echo\"],\"runtime_timeout_seconds\":null,\"runtime_env_name\":null,\"runtime_env_prefix\":null,\"runtime_env_file\":null,\"runtime_runner\":null,\"source_format\":\"agentflow.tool.v0.simple_yaml\",\"source_text\":\"source\"}}}"
        );

        let payload: ToolRegisteredPayload = serde_json::from_str(
            "{\"tool_ref\":\"marker/scan\",\"version\":\"0.1.0\",\"spec_hash\":\"abc123\"}",
        )
        .unwrap();
        assert_eq!(payload.tool_ref, "marker/scan");
    }
}
