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

use super::{ContainerEngineKind, ContainerEngineSelection, PreparedRuntimeCommand};

#[allow(dead_code)]
pub(super) struct ExecContext<'a> {
    pub workdir: &'a Path,
    pub staged_inputs: &'a BTreeMap<String, PathBuf>,
    pub output_dir: &'a Path,
    pub env_names: &'a [String],
    pub container_engine: Option<&'a ContainerEngineSelection>,
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

/// Runs the declared submit command for a detached runtime.
struct DetachedBackend;

impl ToolExecutionBackend for DetachedBackend {
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

/// Runs a Nextflow module through `nextflow run <module.nf> [...]`.
struct NextflowBackend;

impl ToolExecutionBackend for NextflowBackend {
    fn prepare_command(
        &self,
        runtime: &ToolRuntimeSpec,
        _ctx: &ExecContext,
    ) -> Result<PreparedRuntimeCommand, StorageError> {
        let runner = runtime.runner.as_ref().ok_or_else(|| {
            StorageError::InvalidInput("nextflow runtime must declare runner".to_string())
        })?;
        let module = runtime.command.first().ok_or_else(|| {
            StorageError::InvalidInput("runtime.command must not be empty".to_string())
        })?;
        let mut args = vec!["run".to_string(), module.clone()];
        args.extend(runtime.command.iter().skip(1).cloned());
        Ok(PreparedRuntimeCommand {
            executable: runner.clone(),
            args,
        })
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

/// Singularity-compatible container runner. Apptainer uses the same argv shape.
pub(super) struct SingularityEngine;

impl ContainerEngine for SingularityEngine {
    fn build(
        &self,
        runner: &str,
        image: &str,
        command: &[String],
        ctx: &ExecContext,
    ) -> PreparedRuntimeCommand {
        let workdir = ctx.workdir.display().to_string();
        let mut args = vec![
            "exec".to_string(),
            "--containall".to_string(),
            "--net".to_string(),
            "--network".to_string(),
            "none".to_string(),
            "-B".to_string(),
            format!("{workdir}:{workdir}"),
            "--pwd".to_string(),
            workdir,
            image.to_string(),
        ];
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
        let runner_override = ctx
            .container_engine
            .and_then(|selection| selection.runner.as_ref())
            .map(|runner| runner.to_string_lossy().into_owned());
        let runner = match runner_override.as_deref() {
            Some(runner) => runner,
            None => runtime.runner.as_deref().ok_or_else(|| {
                StorageError::InvalidInput(
                    "container runtime must declare runner; pass --container-runner or declare runtime.runner".to_string(),
                )
            })?,
        };
        let image = runtime.image.as_ref().ok_or_else(|| {
            StorageError::InvalidInput("container runtime must declare image".to_string())
        })?;
        let engine_kind = ctx
            .container_engine
            .map(|selection| selection.kind)
            .unwrap_or(ContainerEngineKind::Docker);
        match engine_kind {
            ContainerEngineKind::Docker | ContainerEngineKind::Podman => {
                Ok(DockerEngine.build(runner, image, &runtime.command, ctx))
            }
            ContainerEngineKind::Singularity => {
                Ok(SingularityEngine.build(runner, image, &runtime.command, ctx))
            }
        }
    }
}

/// Routes a backend identifier to its implementation. Unknown backends return
/// `None`; the caller maps that to the existing "unsupported runtime.backend"
/// error so behavior is unchanged.
pub(super) fn backend_for(backend: &str) -> Option<Box<dyn ToolExecutionBackend>> {
    match backend {
        "local" => Some(Box::new(LocalBackend)),
        "detached" => Some(Box::new(DetachedBackend)),
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
        "nextflow" => Some(Box::new(NextflowBackend)),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum DetachedStatus {
    Running,
    Succeeded,
    Failed,
}

#[allow(dead_code)]
pub(crate) fn parse_job_handle(stdout: &str) -> Option<String> {
    stdout.lines().rev().find_map(|line| {
        let value = line.trim().strip_prefix("job_handle=")?;
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    })
}

#[allow(dead_code)]
pub(crate) fn parse_detached_status(stdout: &str) -> Option<DetachedStatus> {
    stdout.lines().rev().find_map(|line| {
        let value = line.trim().strip_prefix("status=")?;
        if value.eq_ignore_ascii_case("running") {
            Some(DetachedStatus::Running)
        } else if value.eq_ignore_ascii_case("succeeded") {
            Some(DetachedStatus::Succeeded)
        } else if value.eq_ignore_ascii_case("failed") {
            Some(DetachedStatus::Failed)
        } else {
            None
        }
    })
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
            container_engine: None,
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

    #[test]
    fn container_backend_podman_selection_uses_docker_argv_shape_with_override_runner() {
        let runtime = ToolRuntimeSpec {
            backend: "container".to_string(),
            command: vec!["python".to_string(), "tool.py".to_string()],
            poll: Vec::new(),
            timeout_seconds: None,
            env_name: None,
            env_prefix: None,
            env_file: None,
            runner: Some("/usr/bin/docker".to_string()),
            image: Some("ghcr.io/acme/tool:1".to_string()),
        };
        let selection = super::ContainerEngineSelection {
            kind: super::ContainerEngineKind::Podman,
            runner: Some(PathBuf::from("/usr/bin/podman")),
        };
        let staged_inputs = BTreeMap::new();
        let env_names = vec!["AGENTFLOW_WORKDIR".to_string()];
        let ctx = ExecContext {
            workdir: Path::new("/tmp/af-step-work"),
            staged_inputs: &staged_inputs,
            output_dir: Path::new("/tmp/af-step-work/outputs"),
            env_names: &env_names,
            container_engine: Some(&selection),
        };

        let prepared = ContainerBackend.prepare_command(&runtime, &ctx).unwrap();

        assert_eq!(prepared.executable, "/usr/bin/podman");
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
                "ghcr.io/acme/tool:1",
                "python",
                "tool.py",
            ]
        );
    }

    #[test]
    fn container_backend_missing_runner_points_to_run_override_or_runtime_runner() {
        let runtime = ToolRuntimeSpec {
            backend: "container".to_string(),
            command: vec!["python".to_string(), "tool.py".to_string()],
            poll: Vec::new(),
            timeout_seconds: None,
            env_name: None,
            env_prefix: None,
            env_file: None,
            runner: None,
            image: Some("ghcr.io/acme/tool:1".to_string()),
        };
        let staged_inputs = BTreeMap::new();
        let ctx = ExecContext {
            workdir: Path::new("/tmp/af-step-work"),
            staged_inputs: &staged_inputs,
            output_dir: Path::new("/tmp/af-step-work/outputs"),
            env_names: &[],
            container_engine: None,
        };

        let err = ContainerBackend
            .prepare_command(&runtime, &ctx)
            .unwrap_err();

        assert!(err
            .to_string()
            .contains("pass --container-runner or declare runtime.runner"));
    }

    #[test]
    fn singularity_engine_builds_exec_argv_without_docker_env_flags() {
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
            container_engine: None,
        };
        let command = vec![
            "python".to_string(),
            "tool.py".to_string(),
            "--mode".to_string(),
            "strict".to_string(),
        ];

        let prepared = SingularityEngine.build(
            "/usr/bin/apptainer",
            "docker://ghcr.io/acme/tool:1",
            &command,
            &ctx,
        );

        assert_eq!(prepared.executable, "/usr/bin/apptainer");
        assert_eq!(
            prepared.args,
            vec![
                "exec",
                "--containall",
                "--net",
                "--network",
                "none",
                "-B",
                "/tmp/af-step-work:/tmp/af-step-work",
                "--pwd",
                "/tmp/af-step-work",
                "docker://ghcr.io/acme/tool:1",
                "python",
                "tool.py",
                "--mode",
                "strict",
            ]
        );
        assert!(!prepared.args.iter().any(|arg| arg == "run"));
        assert!(!prepared.args.iter().any(|arg| arg == "-e"));
        assert_eq!(
            &prepared.args[prepared.args.len() - command.len()..],
            command.as_slice()
        );
    }

    #[test]
    fn nextflow_backend_builds_run_argv() {
        let runtime = ToolRuntimeSpec {
            backend: "nextflow".to_string(),
            command: vec![
                "/abs/mod.nf".to_string(),
                "-profile".to_string(),
                "standard".to_string(),
            ],
            poll: Vec::new(),
            timeout_seconds: None,
            env_name: None,
            env_prefix: None,
            env_file: None,
            runner: Some("/usr/local/bin/nextflow".to_string()),
            image: None,
        };
        let staged_inputs = BTreeMap::new();
        let ctx = ExecContext {
            workdir: Path::new("/tmp/af-step-work"),
            staged_inputs: &staged_inputs,
            output_dir: Path::new("/tmp/af-step-work/outputs"),
            env_names: &[],
            container_engine: None,
        };

        let prepared = NextflowBackend.prepare_command(&runtime, &ctx).unwrap();

        assert_eq!(prepared.executable, "/usr/local/bin/nextflow");
        assert_eq!(
            prepared.args,
            vec!["run", "/abs/mod.nf", "-profile", "standard"]
        );
    }

    #[test]
    fn nextflow_backend_missing_runner_is_error() {
        let runtime = ToolRuntimeSpec {
            backend: "nextflow".to_string(),
            command: vec!["/abs/mod.nf".to_string()],
            poll: Vec::new(),
            timeout_seconds: None,
            env_name: None,
            env_prefix: None,
            env_file: None,
            runner: None,
            image: None,
        };
        let staged_inputs = BTreeMap::new();
        let ctx = ExecContext {
            workdir: Path::new("/tmp/af-step-work"),
            staged_inputs: &staged_inputs,
            output_dir: Path::new("/tmp/af-step-work/outputs"),
            env_names: &[],
            container_engine: None,
        };

        let err = NextflowBackend.prepare_command(&runtime, &ctx).unwrap_err();

        assert!(err
            .to_string()
            .contains("nextflow runtime must declare runner"));
    }

    #[test]
    fn test_parse_job_handle_single() {
        assert_eq!(
            parse_job_handle("job_handle=abc123"),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn test_parse_job_handle_with_surrounding_lines() {
        assert_eq!(
            parse_job_handle("submitting\njob_handle=abc123\nsubmitted"),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn test_parse_job_handle_missing() {
        assert_eq!(parse_job_handle("submitted job abc123"), None);
    }

    #[test]
    fn test_parse_job_handle_empty_value() {
        assert_eq!(parse_job_handle("job_handle="), None);
    }

    #[test]
    fn test_parse_job_handle_trailing_whitespace() {
        assert_eq!(
            parse_job_handle("job_handle=abc  "),
            Some("abc".to_string())
        );
    }

    #[test]
    fn test_parse_detached_status_running() {
        assert_eq!(
            parse_detached_status("status=running"),
            Some(DetachedStatus::Running)
        );
    }

    #[test]
    fn test_parse_detached_status_succeeded() {
        assert_eq!(
            parse_detached_status("status=succeeded"),
            Some(DetachedStatus::Succeeded)
        );
    }

    #[test]
    fn test_parse_detached_status_failed() {
        assert_eq!(
            parse_detached_status("status=failed"),
            Some(DetachedStatus::Failed)
        );
    }

    #[test]
    fn test_parse_detached_status_mixed_case() {
        assert_eq!(
            parse_detached_status("status=Running"),
            Some(DetachedStatus::Running)
        );
    }

    #[test]
    fn test_parse_detached_status_missing() {
        assert_eq!(parse_detached_status("state=running"), None);
    }

    #[test]
    fn test_detached_backend_prepare_command() {
        let runtime = ToolRuntimeSpec {
            backend: "detached".to_string(),
            command: vec!["/abs/submit.sh".to_string(), "--x".to_string()],
            poll: vec!["/abs/poll.sh".to_string()],
            timeout_seconds: None,
            env_name: None,
            env_prefix: None,
            env_file: None,
            runner: None,
            image: None,
        };
        let staged_inputs = BTreeMap::new();
        let ctx = ExecContext {
            workdir: Path::new("/tmp/af-step-work"),
            staged_inputs: &staged_inputs,
            output_dir: Path::new("/tmp/af-step-work/outputs"),
            env_names: &[],
            container_engine: None,
        };

        let prepared = DetachedBackend.prepare_command(&runtime, &ctx).unwrap();

        assert_eq!(prepared.executable, "/abs/submit.sh");
        assert_eq!(prepared.args, vec!["--x"]);
    }
}
