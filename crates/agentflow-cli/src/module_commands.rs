use agentflow_core::storage::{ModuleSpec, ModuleSummary, ProjectStore};

use crate::cli_args::{
    ModuleArgs, ModuleCommand, ModuleFileArgs, ModuleListArgs, ModuleRegisterArgs,
};
use crate::{project_path_from_json, project_path_from_only, CliError};

pub(crate) fn module_command(args: ModuleArgs) -> Result<String, CliError> {
    match args.command {
        ModuleCommand::Register(args) => module_register_command(args),
        ModuleCommand::List(args) => module_list_command(args),
        ModuleCommand::Validate(args) => module_validate_command(args),
        ModuleCommand::Show(args) => module_show_command(args),
    }
}

fn module_register_command(args: ModuleRegisterArgs) -> Result<String, CliError> {
    let source = std::fs::read_to_string(&args.module_yaml)?;
    let spec = ModuleSpec::from_simple_yaml(&source)?;
    let project_path = project_path_from_only(args.project)?;
    let store = ProjectStore::open(&project_path)?;
    let registration = store.register_module(spec)?;
    let action = if registration.replaced_existing {
        "Updated"
    } else {
        "Registered"
    };

    Ok(format!(
        "{action} module\nRef: {}\nVersion: {}\nSpec hash: {}",
        registration.module_ref, registration.version, registration.spec_hash
    ))
}

fn module_list_command(args: ModuleListArgs) -> Result<String, CliError> {
    let project = args.project;
    let json = project.json;
    let project_path = project_path_from_json(project)?;
    let store = ProjectStore::open(&project_path)?;
    let modules = store.list_modules()?;

    if json {
        Ok(modules_list_json(&modules))
    } else if modules.is_empty() {
        Ok("No modules registered".to_string())
    } else {
        Ok(modules
            .iter()
            .map(|module| {
                format!(
                    "{}@{} - {}",
                    module.module_ref, module.version, module.description
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn module_validate_command(args: ModuleFileArgs) -> Result<String, CliError> {
    let source = std::fs::read_to_string(&args.path)?;
    let spec = ModuleSpec::from_simple_yaml(&source)?;

    Ok(format!(
        "Module {} is valid\nVersion: {}\nSteps: {}\nInputs: {}\nOutputs: {}",
        spec.module_ref(),
        spec.version,
        spec.steps.len(),
        spec.inputs.len(),
        spec.outputs.len()
    ))
}

fn module_show_command(args: ModuleFileArgs) -> Result<String, CliError> {
    let source = std::fs::read_to_string(&args.path)?;
    let spec = ModuleSpec::from_simple_yaml(&source)?;

    Ok(format!(
        concat!(
            "Module: {}\n",
            "Version: {}\n",
            "Description: {}\n",
            "Inputs:\n",
            "{}\n",
            "Outputs:\n",
            "{}\n",
            "Steps:\n",
            "{}"
        ),
        spec.module_ref(),
        spec.version,
        spec.description,
        format_module_inputs(&spec),
        format_module_outputs(&spec),
        format_module_steps(&spec)
    ))
}

fn format_module_inputs(spec: &ModuleSpec) -> String {
    if spec.inputs.is_empty() {
        return "  _none_".to_string();
    }

    spec.inputs
        .iter()
        .map(|(name, port)| format!("  {name}: {}", port.type_name))
        .collect::<Vec<_>>()
        .join("\n")
}

fn modules_list_json(modules: &[ModuleSummary]) -> String {
    let items = modules
        .iter()
        .map(|module| {
            format!(
                concat!(
                    "{{",
                    "\"ref\":\"{}\",",
                    "\"namespace\":\"{}\",",
                    "\"name\":\"{}\",",
                    "\"version\":\"{}\",",
                    "\"description\":\"{}\"",
                    "}}"
                ),
                crate::escape_json(&module.module_ref),
                crate::escape_json(&module.namespace),
                crate::escape_json(&module.name),
                crate::escape_json(&module.version),
                crate::escape_json(&module.description)
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    format!(
        "{{\"schema_version\":\"{}\",\"modules\":[{items}]}}",
        agentflow_schemas::MODULE_LIST_JSON_SCHEMA_V0
    )
}

fn format_module_outputs(spec: &ModuleSpec) -> String {
    if spec.outputs.is_empty() {
        return "  _none_".to_string();
    }

    spec.outputs
        .iter()
        .map(|(name, output)| format!("  {name}: {} <- {}", output.type_name, output.from))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_module_steps(spec: &ModuleSpec) -> String {
    if spec.steps.is_empty() {
        return "  _none_".to_string();
    }

    spec.steps
        .iter()
        .map(|step| {
            let needs = if step.needs.is_empty() {
                "none".to_string()
            } else {
                step.needs.join(", ")
            };
            format!("  {}: {} (needs: {needs})", step.id, step.tool_ref)
        })
        .collect::<Vec<_>>()
        .join("\n")
}
