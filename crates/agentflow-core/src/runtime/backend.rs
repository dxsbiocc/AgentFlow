//! Tool execution backends.
//!
//! The runtime dispatches command construction on `ToolRuntimeSpec.backend`.
//! This module hides that dispatch behind the [`ToolExecutionBackend`] trait so
//! future isolated/container backends (see the isolated-execution RFC) can be
//! added without touching the call sites. P1.1 is a behavior-preserving
//! extraction of `prepare_runtime_command`; the produced argv, error text, and
//! cache-relevant config are byte-identical to the previous inline `match`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::storage::{StorageError, ToolRuntimeSpec};

use super::PreparedRuntimeCommand;

#[allow(dead_code)]
pub(super) struct ExecContext<'a> {
    pub workdir: &'a Path,
    pub staged_inputs: &'a BTreeMap<String, PathBuf>,
    pub output_dir: &'a Path,
    pub env_names: &'a [String],
}

/// Builds the concrete executable + argv for a tool run, per backend.
pub(super) trait ToolExecutionBackend {
    fn prepare_command(
        &self,
        runtime: &ToolRuntimeSpec,
        ctx: &ExecContext,
    ) -> Result<PreparedRuntimeCommand, StorageError>;
}

/// Runs the declared command directly, with no environment wrapper.
struct LocalBackend;

impl ToolExecutionBackend for LocalBackend {
    fn prepare_command(
        &self,
        runtime: &ToolRuntimeSpec,
        _ctx: &ExecContext,
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
        _ctx: &ExecContext,
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
        ctx: &ExecContext,
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
        .prepare_command(runtime, ctx)
    }
}

pub(super) trait ContainerEngine {
    fn build(
        &self,
        runner: &str,
        image: &str,
        command: &[String],
        ctx: &ExecContext,
    ) -> PreparedRuntimeCommand;
}

/// Docker-compatible container runner. Podman is supported through the same
/// CLI-compatible argv shape.
pub(super) struct DockerEngine;

impl ContainerEngine for DockerEngine {
    fn build(
        &self,
        runner: &str,
        image: &str,
        command: &[String],
        ctx: &ExecContext,
    ) -> PreparedRuntimeCommand {
        let workdir = ctx.workdir.display().to_string();
        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "--network".to_string(),
            "none".to_string(),
            "-v".to_string(),
            format!("{workdir}:{workdir}"),
            "-w".to_string(),
            workdir,
        ];
        for name in ctx.env_names {
            args.push("-e".to_string());
            args.push(name.clone());
        }
        args.push(image.to_string());
        args.extend(command.iter().cloned());
        PreparedRuntimeCommand {
            executable: runner.to_string(),
            args,
        }
    }
}

/// Runs the declared command inside a container with hard local containment.
struct ContainerBackend;

impl ToolExecutionBackend for ContainerBackend {
    fn prepare_command(
        &self,
        runtime: &ToolRuntimeSpec,
        ctx: &ExecContext,
    ) -> Result<PreparedRuntimeCommand, StorageError> {
        let runner = runtime.runner.as_ref().ok_or_else(|| {
            StorageError::InvalidInput(
                "container runtime must declare absolute runner path".to_string(),
            )
        })?;
        let image = runtime.image.as_ref().ok_or_else(|| {
            StorageError::InvalidInput("container runtime must declare image".to_string())
        })?;
        Ok(DockerEngine.build(runner, image, &runtime.command, ctx))
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
        "container" => Some(Box::new(ContainerBackend)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use super::*;

    #[test]
    fn docker_engine_builds_container_argv_byte_for_byte() {
        let staged_inputs = BTreeMap::new();
        let env_names = vec![
            "AGENTFLOW_WORKDIR".to_string(),
            "AGENTFLOW_INPUT_READS".to_string(),
            "AGENTFLOW_PARAMS_JSON".to_string(),
            "AGENTFLOW_OUTPUT_REPORT".to_string(),
        ];
        let ctx = ExecContext {
            workdir: Path::new("/tmp/af-step-work"),
            staged_inputs: &staged_inputs,
            output_dir: Path::new("/tmp/af-step-work/outputs"),
            env_names: &env_names,
        };
        let command = vec![
            "python".to_string(),
            "tool.py".to_string(),
            "--mode".to_string(),
            "strict".to_string(),
        ];

        let prepared = DockerEngine.build(
            "/usr/bin/docker",
            "ghcr.io/acme/tool@sha256:0123456789abcdef",
            &command,
            &ctx,
        );

        assert_eq!(prepared.executable, "/usr/bin/docker");
        assert_eq!(
            prepared.args,
            vec![
                "run",
                "--rm",
                "--network",
                "none",
                "-v",
                "/tmp/af-step-work:/tmp/af-step-work",
                "-w",
                "/tmp/af-step-work",
                "-e",
                "AGENTFLOW_WORKDIR",
                "-e",
                "AGENTFLOW_INPUT_READS",
                "-e",
                "AGENTFLOW_PARAMS_JSON",
                "-e",
                "AGENTFLOW_OUTPUT_REPORT",
                "ghcr.io/acme/tool@sha256:0123456789abcdef",
                "python",
                "tool.py",
                "--mode",
                "strict",
            ]
        );
        assert_eq!(
            prepared.argv(),
            vec![
                "/usr/bin/docker",
                "run",
                "--rm",
                "--network",
                "none",
                "-v",
                "/tmp/af-step-work:/tmp/af-step-work",
                "-w",
                "/tmp/af-step-work",
                "-e",
                "AGENTFLOW_WORKDIR",
                "-e",
                "AGENTFLOW_INPUT_READS",
                "-e",
                "AGENTFLOW_PARAMS_JSON",
                "-e",
                "AGENTFLOW_OUTPUT_REPORT",
                "ghcr.io/acme/tool@sha256:0123456789abcdef",
                "python",
                "tool.py",
                "--mode",
                "strict",
            ]
        );
    }
}
