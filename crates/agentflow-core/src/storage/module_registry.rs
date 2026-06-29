//! First-class modules: reusable, typed sub-flows.
//!
//! A module is a named, versioned, reusable piece of a flow — declared external
//! input/output ports plus internal steps (ordinary [`FlowStepDraft`]s) wired
//! together. It is composed into a flow by *inline expansion*: the internal
//! steps are copied into the flow with their ids and internal artifact names
//! namespaced to one instance, the external input ports rewired to the caller's
//! bound artifacts, and the external output ports exposed back to the caller.
//!
//! Expansion is a pure transformation over flow drafts, so the existing
//! scheduler and runtime execute the flattened DAG with no changes. Storage,
//! CLI, and agent composition are built on this primitive in later slices.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{params, OptionalExtension};

use super::flow_registry::FlowStepDraft;
use super::migrations;
use super::project_store::{now_unix_seconds, ProjectStore, StorageError};
use super::yaml;

const DEFAULT_NAMESPACE: &str = "local";

/// A typed external port of a module (input or, with `from`, output).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModulePort {
    pub type_name: String,
}

/// A typed external output port, bound to an internal artifact name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleOutput {
    pub type_name: String,
    /// The internal artifact name (produced by one of the module's steps) that
    /// this output port exposes.
    pub from: String,
}

/// A reusable, typed sub-flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSpec {
    pub schema_version: String,
    pub namespace: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub inputs: BTreeMap<String, ModulePort>,
    pub outputs: BTreeMap<String, ModuleOutput>,
    pub steps: Vec<FlowStepDraft>,
    pub source_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleRegistration {
    pub module_ref: String,
    pub version: String,
    pub spec_hash: String,
    pub replaced_existing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSummary {
    pub module_ref: String,
    pub namespace: String,
    pub name: String,
    pub version: String,
    pub description: String,
}

/// The result of expanding one module instance into a flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleExpansion {
    /// Inlined steps, with ids and internal artifact names namespaced to the
    /// instance and external inputs rewired to the caller's bound values.
    pub steps: Vec<FlowStepDraft>,
    /// Map of the module's external output port name -> the (namespaced)
    /// internal artifact name that carries it, for the caller to wire onward.
    pub outputs: BTreeMap<String, String>,
}

impl ProjectStore {
    pub fn register_module(&self, spec: ModuleSpec) -> Result<ModuleRegistration, StorageError> {
        let module_ref = spec.module_ref();
        let spec_hash = migrations::checksum(&spec.source_text);
        let now = now_unix_seconds();

        // `replaced_existing` is read non-atomically before the upsert (matching
        // the tool registry). A ProjectStore owns a single connection used
        // serially, so the flag is reliable in practice; the INSERT ... ON
        // CONFLICT below is authoritative regardless.
        let existing = self
            .connection()
            .query_row(
                "SELECT id FROM modules WHERE id = ?1",
                params![&module_ref],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let replaced_existing = existing.is_some();

        self.connection().execute(
            "INSERT INTO modules
             (id, namespace, name, version, schema_version, description, source_text, spec_hash,
              created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
               namespace = excluded.namespace,
               name = excluded.name,
               version = excluded.version,
               schema_version = excluded.schema_version,
               description = excluded.description,
               source_text = excluded.source_text,
               spec_hash = excluded.spec_hash,
               updated_at = excluded.updated_at",
            params![
                &module_ref,
                &spec.namespace,
                &spec.name,
                &spec.version,
                &spec.schema_version,
                &spec.description,
                &spec.source_text,
                &spec_hash,
                now,
                now
            ],
        )?;
        self.touch_project()?;

        Ok(ModuleRegistration {
            module_ref,
            version: spec.version,
            spec_hash,
            replaced_existing,
        })
    }

    pub fn list_modules(&self) -> Result<Vec<ModuleSummary>, StorageError> {
        let mut stmt = self.connection().prepare(
            "SELECT id, namespace, name, version, description
             FROM modules
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ModuleSummary {
                module_ref: row.get(0)?,
                namespace: row.get(1)?,
                name: row.get(2)?,
                version: row.get(3)?,
                description: row.get(4)?,
            })
        })?;

        let mut modules = Vec::new();
        for row in rows {
            modules.push(row?);
        }
        Ok(modules)
    }

    pub fn get_module(&self, module_ref: &str) -> Result<ModuleSpec, StorageError> {
        let source_text = self
            .connection()
            .query_row(
                "SELECT source_text FROM modules WHERE id = ?1",
                params![module_ref],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(format!("module {module_ref}")))?;

        ModuleSpec::from_simple_yaml(&source_text)
    }
}

impl ModuleSpec {
    pub fn module_ref(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }

    pub fn from_simple_yaml(source_text: &str) -> Result<Self, StorageError> {
        let raw = yaml::parse_yaml::<RawModuleSpec>("module", source_text)?;
        let schema_version =
            required_field(raw.schema_version.clone(), "schema_version", source_text)?;
        if schema_version != agentflow_schemas::MODULE_SCHEMA_V0 {
            return Err(yaml::invalid_input_at_field(
                source_text,
                "schema_version",
                format!(
                    "module schema_version must be {}",
                    agentflow_schemas::MODULE_SCHEMA_V0
                ),
            ));
        }

        let namespace = raw
            .namespace
            .clone()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_NAMESPACE.to_string());
        let name = required_field(raw.name.clone(), "name", source_text)?;
        let version = required_field(raw.version.clone(), "version", source_text)?;
        let description = required_field(raw.description.clone(), "description", source_text)?;
        validate_ident("namespace", &namespace)?;
        validate_ident("name", &name)?;
        validate_ident("version", &version)?;

        let inputs = raw
            .inputs
            .into_iter()
            .map(|(name, port)| {
                (
                    name,
                    ModulePort {
                        type_name: port.r#type,
                    },
                )
            })
            .collect();
        let outputs = raw
            .outputs
            .into_iter()
            .map(|(name, port)| {
                (
                    name,
                    ModuleOutput {
                        type_name: port.r#type,
                        from: port.from,
                    },
                )
            })
            .collect();
        let steps = raw
            .steps
            .into_iter()
            .map(RawModuleStep::into_step)
            .collect::<Result<Vec<_>, _>>()?;

        let spec = Self {
            schema_version,
            namespace,
            name,
            version,
            description,
            inputs,
            outputs,
            steps,
            source_text: source_text.to_string(),
        };
        spec.validate()?;
        Ok(spec)
    }

    /// Structural validation: ids unique, ports/outputs well-typed, every output
    /// port resolves to an internal artifact, and every internal step input
    /// resolves to either an external input port or an internal artifact (no
    /// dangling references — a module carries all external data through ports).
    pub fn validate(&self) -> Result<(), StorageError> {
        if self.steps.is_empty() {
            return Err(StorageError::InvalidInput(
                "module must declare at least one step".to_string(),
            ));
        }
        let mut step_ids = BTreeSet::new();
        for step in &self.steps {
            validate_ident("module step id", &step.id)?;
            if !step_ids.insert(step.id.clone()) {
                return Err(StorageError::InvalidInput(format!(
                    "module step id {} is declared more than once",
                    step.id
                )));
            }
            if step.tool_ref.trim().is_empty() {
                return Err(StorageError::InvalidInput(format!(
                    "module step {} must declare a tool",
                    step.id
                )));
            }
        }

        // Map each internal artifact name to the step that produces it, rejecting
        // two steps that emit the same name (which would make wiring ambiguous).
        let mut artifact_producer: BTreeMap<&str, &str> = BTreeMap::new();
        for step in &self.steps {
            for artifact in step.outputs.values() {
                validate_ident("module step output artifact name", artifact)?;
                if let Some(other) = artifact_producer.insert(artifact.as_str(), step.id.as_str()) {
                    return Err(StorageError::InvalidInput(format!(
                        "module artifact {artifact} is produced by both step {other} and step {}",
                        step.id
                    )));
                }
            }
        }

        let port_names: BTreeSet<&str> = self.inputs.keys().map(String::as_str).collect();

        for (name, port) in &self.inputs {
            validate_ident("module input port", name)?;
            if port.type_name.trim().is_empty() {
                return Err(StorageError::InvalidInput(format!(
                    "module input port {name} must declare type"
                )));
            }
            // The wiring convention has no sigil: an input value is a port iff it
            // equals a port name, otherwise an internal artifact. Identical names
            // would be ambiguous, so ban the collision.
            if artifact_producer.contains_key(name.as_str()) {
                return Err(StorageError::InvalidInput(format!(
                    "module input port {name} collides with an internal artifact of the same \
                     name; rename one to disambiguate"
                )));
            }
        }
        for (name, output) in &self.outputs {
            validate_ident("module output port", name)?;
            if output.type_name.trim().is_empty() {
                return Err(StorageError::InvalidInput(format!(
                    "module output port {name} must declare type"
                )));
            }
            if !artifact_producer.contains_key(output.from.as_str()) {
                return Err(StorageError::InvalidInput(format!(
                    "module output port {name} maps to {}, which no step produces",
                    output.from
                )));
            }
        }

        for step in &self.steps {
            for need in &step.needs {
                if !step_ids.contains(need) {
                    return Err(StorageError::InvalidInput(format!(
                        "module step {} needs {need}, which is not a step in this module",
                        step.id
                    )));
                }
            }
            for (input_name, value) in &step.inputs {
                if port_names.contains(value.as_str()) {
                    continue;
                }
                // Otherwise it must be an artifact produced inside the module, and
                // the consuming step must declare the producer in `needs` so the
                // scheduler runs them in order once expanded into a flow.
                let Some(&producer) = artifact_producer.get(value.as_str()) else {
                    return Err(StorageError::InvalidInput(format!(
                        "module step {} input {input_name} references {value}, which is neither \
                         a declared input port nor an artifact produced inside the module",
                        step.id
                    )));
                };
                if !step.needs.iter().any(|need| need == producer) {
                    return Err(StorageError::InvalidInput(format!(
                        "module step {} uses artifact {value} from step {producer} but does not \
                         declare needs: [{producer}]",
                        step.id
                    )));
                }
            }
        }

        if let Some(cycle_step) = first_step_in_a_cycle(&self.steps) {
            return Err(StorageError::InvalidInput(format!(
                "module step needs form a dependency cycle (involving {cycle_step})"
            )));
        }
        Ok(())
    }

    /// Expand one instance of this module into flow steps. `instance_id`
    /// namespaces every internal id/artifact (so the same module can appear more
    /// than once in a flow); `input_bindings` maps each declared input port to
    /// the caller's artifact reference or value.
    ///
    /// Assumes a structurally valid [`ModuleSpec`] (guaranteed by
    /// [`ModuleSpec::from_simple_yaml`]); only the per-call binding set is
    /// re-checked here.
    pub fn expand(
        &self,
        instance_id: &str,
        input_bindings: &BTreeMap<String, String>,
    ) -> Result<ModuleExpansion, StorageError> {
        validate_ident("module instance id", instance_id)?;

        for port in self.inputs.keys() {
            if !input_bindings.contains_key(port) {
                return Err(StorageError::InvalidInput(format!(
                    "module instance {instance_id} is missing a binding for input port {port}"
                )));
            }
        }
        for bound in input_bindings.keys() {
            if !self.inputs.contains_key(bound) {
                return Err(StorageError::InvalidInput(format!(
                    "module instance {instance_id} binds unknown input port {bound}"
                )));
            }
        }

        let port_names: BTreeSet<&str> = self.inputs.keys().map(String::as_str).collect();
        let prefix = |local: &str| format!("{instance_id}__{local}");

        let steps = self
            .steps
            .iter()
            .map(|step| {
                let needs = step.needs.iter().map(|need| prefix(need)).collect();
                let inputs = step
                    .inputs
                    .iter()
                    .map(|(input_name, value)| {
                        let resolved = if port_names.contains(value.as_str()) {
                            input_bindings
                                .get(value)
                                .expect("required port binding present after the check above")
                                .clone()
                        } else {
                            // internal artifact — namespace it to this instance
                            prefix(value)
                        };
                        (input_name.clone(), resolved)
                    })
                    .collect();
                let outputs = step
                    .outputs
                    .iter()
                    .map(|(output_name, artifact)| (output_name.clone(), prefix(artifact)))
                    .collect();
                FlowStepDraft {
                    id: prefix(&step.id),
                    tool_ref: step.tool_ref.clone(),
                    needs,
                    reason: step.reason.clone(),
                    inputs,
                    params: step.params.clone(),
                    outputs,
                }
            })
            .collect();

        let outputs = self
            .outputs
            .iter()
            .map(|(port, output)| (port.clone(), prefix(&output.from)))
            .collect();

        Ok(ModuleExpansion { steps, outputs })
    }
}

/// Returns the id of a step that participates in a `needs` cycle, if any.
/// Assumes every `needs` entry names an existing step (checked by `validate`).
fn first_step_in_a_cycle(steps: &[FlowStepDraft]) -> Option<String> {
    let needs: BTreeMap<&str, &Vec<String>> =
        steps.iter().map(|s| (s.id.as_str(), &s.needs)).collect();
    let mut visited: BTreeSet<&str> = BTreeSet::new();
    let mut on_stack: BTreeSet<&str> = BTreeSet::new();

    fn visit<'a>(
        id: &'a str,
        needs: &BTreeMap<&'a str, &'a Vec<String>>,
        visited: &mut BTreeSet<&'a str>,
        on_stack: &mut BTreeSet<&'a str>,
    ) -> Option<String> {
        if on_stack.contains(id) {
            return Some(id.to_string());
        }
        if !visited.insert(id) {
            return None;
        }
        on_stack.insert(id);
        if let Some(deps) = needs.get(id) {
            for dep in deps.iter() {
                if let Some(found) = visit(dep, needs, visited, on_stack) {
                    return Some(found);
                }
            }
        }
        on_stack.remove(id);
        None
    }

    for step in steps {
        if let Some(found) = visit(step.id.as_str(), &needs, &mut visited, &mut on_stack) {
            return Some(found);
        }
    }
    None
}

fn required_field(
    value: Option<String>,
    field_name: &str,
    source_text: &str,
) -> Result<String, StorageError> {
    value.filter(|value| !value.is_empty()).ok_or_else(|| {
        yaml::invalid_input_at_field(
            source_text,
            field_name,
            format!("module spec is missing required field {field_name}"),
        )
    })
}

/// Identifier rule shared across module names, ports, and step ids: ASCII
/// letters, numbers, underscore, dash, and dot (matching flow/tool refs).
fn validate_ident(label: &str, value: &str) -> Result<(), StorageError> {
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

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModuleSpec {
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    schema_version: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    namespace: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    name: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    version: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    description: Option<String>,
    #[serde(default)]
    inputs: BTreeMap<String, RawModulePort>,
    #[serde(default)]
    outputs: BTreeMap<String, RawModuleOutput>,
    #[serde(default, deserialize_with = "yaml::deserialize_default_vec")]
    steps: Vec<RawModuleStep>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModulePort {
    #[serde(default)]
    r#type: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModuleOutput {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    from: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModuleStep {
    #[serde(default, deserialize_with = "yaml::deserialize_optional_scalar_string")]
    id: Option<String>,
    #[serde(
        rename = "tool",
        default,
        deserialize_with = "yaml::deserialize_optional_scalar_string"
    )]
    tool_ref: Option<String>,
    #[serde(
        default,
        deserialize_with = "yaml::deserialize_optional_present_scalar_string"
    )]
    reason: Option<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_string_vec_or_csv")]
    needs: Vec<String>,
    #[serde(default, deserialize_with = "yaml::deserialize_string_map")]
    inputs: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "yaml::deserialize_string_map")]
    params: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "yaml::deserialize_string_map")]
    outputs: BTreeMap<String, String>,
}

impl RawModuleStep {
    fn into_step(self) -> Result<FlowStepDraft, StorageError> {
        let id = self.id.unwrap_or_default();
        if id.trim().is_empty() {
            return Err(StorageError::InvalidInput(
                "module step is missing id".to_string(),
            ));
        }
        Ok(FlowStepDraft {
            id,
            tool_ref: self.tool_ref.unwrap_or_default(),
            needs: self.needs,
            reason: self.reason,
            inputs: self.inputs,
            params: self.params,
            outputs: self.outputs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_STEP_MODULE: &str = r#"
schema_version: agentflow.module.v0
namespace: bio
name: qc_then_quantify
version: 0.1.0
description: QC raw counts then quantify into an expression table.
inputs:
  counts:
    type: RawCounts
outputs:
  expression:
    type: ExpressionTable
    from: quant_out
steps:
  - id: qc
    tool: bio/qc
    inputs:
      counts: counts
    outputs:
      clean: qc_clean
  - id: quant
    tool: bio/quantify
    needs: [qc]
    inputs:
      counts: qc_clean
    outputs:
      expression: quant_out
"#;

    fn temp_project_path(test_name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-module-registry-{test_name}-{}-{}",
            std::process::id(),
            now_unix_seconds()
        ))
    }

    fn module_with_version(version: &str) -> String {
        TWO_STEP_MODULE.replace("version: 0.1.0", &format!("version: {version}"))
    }

    #[test]
    fn registers_and_lists_module() {
        let path = temp_project_path("register-list");
        let store = ProjectStore::init(&path, Some("Modules")).unwrap();

        let registration = store
            .register_module(ModuleSpec::from_simple_yaml(TWO_STEP_MODULE).unwrap())
            .unwrap();
        assert_eq!(registration.module_ref, "bio/qc_then_quantify");
        assert_eq!(registration.version, "0.1.0");
        assert!(!registration.replaced_existing);
        assert_eq!(
            registration.spec_hash,
            migrations::checksum(TWO_STEP_MODULE)
        );

        let modules = store.list_modules().unwrap();
        assert_eq!(
            modules,
            vec![ModuleSummary {
                module_ref: "bio/qc_then_quantify".to_string(),
                namespace: "bio".to_string(),
                name: "qc_then_quantify".to_string(),
                version: "0.1.0".to_string(),
                description: "QC raw counts then quantify into an expression table.".to_string(),
            }]
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn get_module_round_trips_registered_source() {
        let path = temp_project_path("get-round-trip");
        let store = ProjectStore::init(&path, Some("Modules")).unwrap();
        let original = ModuleSpec::from_simple_yaml(TWO_STEP_MODULE).unwrap();
        store.register_module(original.clone()).unwrap();

        let stored = store.get_module("bio/qc_then_quantify").unwrap();
        assert_eq!(stored, original);

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn reregistering_same_module_ref_replaces_existing_row() {
        let path = temp_project_path("replace-existing");
        let store = ProjectStore::init(&path, Some("Modules")).unwrap();
        let first = store
            .register_module(ModuleSpec::from_simple_yaml(&module_with_version("0.1.0")).unwrap())
            .unwrap();
        let second = store
            .register_module(ModuleSpec::from_simple_yaml(&module_with_version("0.2.0")).unwrap())
            .unwrap();

        assert!(!first.replaced_existing);
        assert!(second.replaced_existing);

        let modules = store.list_modules().unwrap();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].module_ref, "bio/qc_then_quantify");
        assert_eq!(modules[0].version, "0.2.0");
        assert_eq!(
            store.get_module("bio/qc_then_quantify").unwrap().version,
            "0.2.0"
        );

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn get_module_returns_not_found_for_missing_ref() {
        let path = temp_project_path("missing");
        let store = ProjectStore::init(&path, Some("Modules")).unwrap();

        let err = store.get_module("bio/missing").unwrap_err();
        assert!(matches!(err, StorageError::NotFound(_)));

        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn parses_and_validates_a_module() {
        let module = ModuleSpec::from_simple_yaml(TWO_STEP_MODULE).unwrap();
        assert_eq!(module.module_ref(), "bio/qc_then_quantify");
        assert_eq!(module.steps.len(), 2);
        assert_eq!(module.inputs["counts"].type_name, "RawCounts");
        assert_eq!(module.outputs["expression"].from, "quant_out");
    }

    #[test]
    fn expands_with_prefixed_ids_and_rewired_ports() {
        let module = ModuleSpec::from_simple_yaml(TWO_STEP_MODULE).unwrap();
        let bindings = BTreeMap::from([("counts".to_string(), "artifact_raw123".to_string())]);
        let expansion = module.expand("m1", &bindings).unwrap();

        assert_eq!(expansion.steps.len(), 2);
        let qc = &expansion.steps[0];
        assert_eq!(qc.id, "m1__qc");
        // external input port rewired to the caller's binding
        assert_eq!(qc.inputs["counts"], "artifact_raw123");
        // internal output artifact namespaced to the instance
        assert_eq!(qc.outputs["clean"], "m1__qc_clean");

        let quant = &expansion.steps[1];
        assert_eq!(quant.id, "m1__quant");
        assert_eq!(quant.needs, vec!["m1__qc".to_string()]);
        // internal input wired to the prefixed upstream artifact, not a port
        assert_eq!(quant.inputs["counts"], "m1__qc_clean");
        assert_eq!(quant.outputs["expression"], "m1__quant_out");

        // external output exposes the namespaced internal artifact
        assert_eq!(expansion.outputs["expression"], "m1__quant_out");
    }

    #[test]
    fn two_instances_do_not_collide() {
        let module = ModuleSpec::from_simple_yaml(TWO_STEP_MODULE).unwrap();
        let bindings_a = BTreeMap::from([("counts".to_string(), "artifact_a".to_string())]);
        let bindings_b = BTreeMap::from([("counts".to_string(), "artifact_b".to_string())]);
        let a = module.expand("a", &bindings_a).unwrap();
        let b = module.expand("b", &bindings_b).unwrap();
        assert_eq!(a.steps[0].id, "a__qc");
        assert_eq!(b.steps[0].id, "b__qc");
        assert_eq!(a.outputs["expression"], "a__quant_out");
        assert_eq!(b.outputs["expression"], "b__quant_out");
    }

    #[test]
    fn expand_rejects_missing_and_unknown_bindings() {
        let module = ModuleSpec::from_simple_yaml(TWO_STEP_MODULE).unwrap();
        let missing = module.expand("m1", &BTreeMap::new()).unwrap_err();
        assert!(missing
            .to_string()
            .contains("missing a binding for input port counts"));

        let unknown = module
            .expand(
                "m1",
                &BTreeMap::from([
                    ("counts".to_string(), "artifact_x".to_string()),
                    ("bogus".to_string(), "artifact_y".to_string()),
                ]),
            )
            .unwrap_err();
        assert!(unknown
            .to_string()
            .contains("binds unknown input port bogus"));
    }

    #[test]
    fn rejects_output_port_without_internal_producer() {
        let yaml = r#"
schema_version: agentflow.module.v0
name: bad_out
version: 0.1.0
description: output maps to nothing
inputs:
  counts:
    type: RawCounts
outputs:
  result:
    type: ExpressionTable
    from: nonexistent
steps:
  - id: only
    tool: bio/qc
    inputs:
      counts: counts
    outputs:
      clean: qc_clean
"#;
        let err = ModuleSpec::from_simple_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("which no step produces"));
    }

    #[test]
    fn rejects_internal_input_without_needs_on_producer() {
        // `quant` reads `qc_clean` (produced by `qc`) but omits needs: [qc].
        let yaml = r#"
schema_version: agentflow.module.v0
name: missing_needs
version: 0.1.0
description: consumer omits needs on producer
inputs:
  counts:
    type: RawCounts
outputs:
  expression:
    type: ExpressionTable
    from: quant_out
steps:
  - id: qc
    tool: bio/qc
    inputs:
      counts: counts
    outputs:
      clean: qc_clean
  - id: quant
    tool: bio/quantify
    inputs:
      counts: qc_clean
    outputs:
      expression: quant_out
"#;
        let err = ModuleSpec::from_simple_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("does not declare needs: [qc]"));
    }

    #[test]
    fn rejects_port_colliding_with_internal_artifact() {
        let yaml = r#"
schema_version: agentflow.module.v0
name: collision
version: 0.1.0
description: port name equals an internal artifact name
inputs:
  clean:
    type: RawCounts
outputs:
  expression:
    type: ExpressionTable
    from: quant_out
steps:
  - id: qc
    tool: bio/qc
    inputs:
      counts: clean
    outputs:
      clean: clean
  - id: quant
    tool: bio/quantify
    needs: [qc]
    inputs:
      counts: clean
    outputs:
      expression: quant_out
"#;
        let err = ModuleSpec::from_simple_yaml(yaml).unwrap_err();
        assert!(err
            .to_string()
            .contains("collides with an internal artifact"));
    }

    #[test]
    fn rejects_needs_cycle() {
        let yaml = r#"
schema_version: agentflow.module.v0
name: cyclic
version: 0.1.0
description: a needs cycle
inputs:
  counts:
    type: RawCounts
outputs:
  out:
    type: RawCounts
    from: a_out
steps:
  - id: a
    tool: bio/a
    needs: [b]
    outputs:
      out: a_out
  - id: b
    tool: bio/b
    needs: [a]
    outputs:
      out: b_out
"#;
        let err = ModuleSpec::from_simple_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("dependency cycle"));
    }

    #[test]
    fn rejects_duplicate_artifact_producer() {
        let yaml = r#"
schema_version: agentflow.module.v0
name: dup_artifact
version: 0.1.0
description: two steps produce the same artifact name
inputs:
  counts:
    type: RawCounts
outputs:
  out:
    type: RawCounts
    from: shared
steps:
  - id: a
    tool: bio/a
    inputs:
      counts: counts
    outputs:
      out: shared
  - id: b
    tool: bio/b
    inputs:
      counts: counts
    outputs:
      out: shared
"#;
        let err = ModuleSpec::from_simple_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("produced by both step"));
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let yaml = r#"
schema_version: agentflow.module.v0
name: typo
version: 0.1.0
description: misspelled steps field
inputs:
  counts:
    type: RawCounts
stepz:
  - id: a
    tool: bio/a
"#;
        assert!(ModuleSpec::from_simple_yaml(yaml).is_err());
    }

    #[test]
    fn expands_a_zero_input_module() {
        // A module that sources its own data needs no input ports.
        let yaml = r#"
schema_version: agentflow.module.v0
name: fetch_only
version: 0.1.0
description: fetches data with no external inputs
outputs:
  data:
    type: RawCounts
    from: fetched
steps:
  - id: fetch
    tool: bio/fetch
    outputs:
      data: fetched
"#;
        let module = ModuleSpec::from_simple_yaml(yaml).unwrap();
        let expansion = module.expand("only", &BTreeMap::new()).unwrap();
        assert_eq!(expansion.steps.len(), 1);
        assert_eq!(expansion.steps[0].id, "only__fetch");
        assert_eq!(expansion.outputs["data"], "only__fetched");
    }

    #[test]
    fn rejects_dangling_internal_input_reference() {
        let yaml = r#"
schema_version: agentflow.module.v0
name: bad_input
version: 0.1.0
description: input references an undeclared artifact
inputs:
  counts:
    type: RawCounts
outputs:
  expression:
    type: ExpressionTable
    from: quant_out
steps:
  - id: quant
    tool: bio/quantify
    inputs:
      counts: not_a_port_or_artifact
    outputs:
      expression: quant_out
"#;
        let err = ModuleSpec::from_simple_yaml(yaml).unwrap_err();
        assert!(err
            .to_string()
            .contains("neither a declared input port nor an artifact produced inside the module"));
    }
}
