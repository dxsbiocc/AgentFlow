use agentflow_core::storage::ModuleSpec;

use crate::cli_args::{ModuleArgs, ModuleCommand, ModuleFileArgs};
use crate::CliError;

pub(crate) fn module_command(args: ModuleArgs) -> Result<String, CliError> {
    match args.command {
        ModuleCommand::Validate(args) => module_validate_command(args),
        ModuleCommand::Show(args) => module_show_command(args),
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
