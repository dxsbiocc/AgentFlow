//! Tool execution backends.
//!
//! The runtime dispatches command construction on `ToolRuntimeSpec.backend`.
//! This module hides that dispatch behind the [`ToolExecutionBackend`] trait so
//! future isolated/container backends (see the isolated-execution RFC) can be
//! added without touching the call sites. P1.1 is a behavior-preserving
//! extraction of `prepare_runtime_command`; the produced argv, error text, and
//! cache-relevant config are byte-identical to the previous inline `match`.

use crate::storage::{StorageError, ToolRuntimeSpec};

use super::PreparedRuntimeCommand;

/// Builds the concrete executable + argv for a tool run, per backend.
pub(super) trait ToolExecutionBackend {
    fn prepare_command(
        &self,
        runtime: &ToolRuntimeSpec,
    ) -> Result<PreparedRuntimeCommand, StorageError>;
}

/// Runs the declared command directly, with no environment wrapper.
struct LocalBackend;

impl ToolExecutionBackend for LocalBackend {
    fn prepare_command(
        &self,
        runtime: &ToolRuntimeSpec,
    ) -> Result<PreparedRuntimeCommand, StorageError> {
        let executable = runtime.command.first().ok_or_else(|| {
            StorageError::InvalidInput("runtime.command must not be empty".to_string())
        })?;
        Ok(PreparedRuntimeCommand {
            executable: executable.clone(),
            args: runtime.command.iter().skip(1).cloned().collect(),
        })
    }
}

/// Runs the declared command inside an existing conda/micromamba environment
/// via `<runner> run [...] <command>`. `conda_no_capture` preserves the one
/// historical difference between the `conda` and `micromamba` runners.
struct CondaBackend {
    conda_no_capture: bool,
    prefix_flag: &'static str,
}

impl ToolExecutionBackend for CondaBackend {
    fn prepare_command(
        &self,
        runtime: &ToolRuntimeSpec,
    ) -> Result<PreparedRuntimeCommand, StorageError> {
        let runner = runtime.runner.as_ref().ok_or_else(|| {
            StorageError::InvalidInput(
                "environment runtime must declare absolute runner path".to_string(),
            )
        })?;
        let mut args = vec!["run".to_string()];
        if self.conda_no_capture {
            args.push("--no-capture-output".to_string());
        }
        match (runtime.env_name.as_deref(), runtime.env_prefix.as_deref()) {
            (Some(env_name), None) => {
                args.push("--name".to_string());
                args.push(env_name.to_string());
            }
            (None, Some(env_prefix)) => {
                args.push(self.prefix_flag.to_string());
                args.push(env_prefix.to_string());
            }
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
        }
        args.extend(runtime.command.iter().cloned());
        Ok(PreparedRuntimeCommand {
            executable: runner.clone(),
            args,
        })
    }
}

/// Runs a declared command inside an AgentFlow-managed micromamba prefix. The
/// caller injects the derived managed prefix into `runtime.env_prefix`.
struct IsolatedMicromambaBackend;

impl ToolExecutionBackend for IsolatedMicromambaBackend {
    fn prepare_command(
        &self,
        runtime: &ToolRuntimeSpec,
    ) -> Result<PreparedRuntimeCommand, StorageError> {
        if runtime.env_name.is_some() {
            return Err(StorageError::InvalidInput(
                "isolated runtime must use a managed env_prefix, not env_name".to_string(),
            ));
        }
        if runtime.env_prefix.is_none() {
            return Err(StorageError::InvalidInput(
                "isolated runtime must declare managed env_prefix before command preparation"
                    .to_string(),
            ));
        }
        CondaBackend {
            conda_no_capture: false,
            prefix_flag: "-p",
        }
        .prepare_command(runtime)
    }
}

/// Routes a backend identifier to its implementation. Unknown backends return
/// `None`; the caller maps that to the existing "unsupported runtime.backend"
/// error so behavior is unchanged.
pub(super) fn backend_for(backend: &str) -> Option<Box<dyn ToolExecutionBackend>> {
    match backend {
        "local" => Some(Box::new(LocalBackend)),
        "conda" => Some(Box::new(CondaBackend {
            conda_no_capture: true,
            prefix_flag: "--prefix",
        })),
        "micromamba" => Some(Box::new(CondaBackend {
            conda_no_capture: false,
            prefix_flag: "--prefix",
        })),
        "isolated-micromamba" => Some(Box::new(IsolatedMicromambaBackend)),
        _ => None,
    }
}
