use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::domain::{RunAttemptStatus, StepStatus};
use crate::storage::{
    project_dir, ComputedArtifactRequest, ProjectStore, StorageError, StoredFlowStep,
    ToolRuntimeSpec,
};

mod backend;
mod schedule;

use schedule::{RuleBasedStepScheduler, StepScheduler};

const ISOLATED_ENV_BACKEND: &str = "isolated-micromamba";
const ISOLATED_ENV_LOCK_FILE: &str = "agentflow-env.lock";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerEngineKind {
    Docker,
    Podman,
    Singularity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerEngineSelection {
    pub kind: ContainerEngineKind,
    pub runner: Option<PathBuf>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RunConfig {
    pub container_engine: Option<ContainerEngineSelection>,
    /// Maximum number of independent ready steps whose tool subprocesses may run
    /// concurrently within one scheduler wave. 0 or 1 means sequential (the
    /// default, byte-identical to the pre-parallel runtime). Only the subprocess
    /// overlaps; preparation and recording stay serial on the main thread.
    pub max_parallel: usize,
    /// Continue running independent steps when one fails, instead of stopping the
    /// run at the first failure. A failed step is terminal (not retried) and its
    /// dependents are skipped, but unrelated ready steps still run. Default
    /// `false` keeps the fail-fast behavior, byte-identical to before.
    pub keep_going: bool,
}

thread_local! {
    static SCOPED_RUN_CONFIG: RefCell<Option<RunConfig>> = const { RefCell::new(None) };
}

pub fn with_run_config<T>(config: &RunConfig, run: impl FnOnce() -> T) -> T {
    struct ScopedRunConfigGuard(Option<RunConfig>);

    impl Drop for ScopedRunConfigGuard {
        fn drop(&mut self) {
            let previous = self.0.take();
            SCOPED_RUN_CONFIG.with(|slot| {
                *slot.borrow_mut() = previous;
            });
        }
    }

    let previous = SCOPED_RUN_CONFIG.with(|slot| slot.replace(Some(config.clone())));
    let _guard = ScopedRunConfigGuard(previous);
    run()
}

fn scoped_run_config() -> Option<RunConfig> {
    SCOPED_RUN_CONFIG.with(|slot| slot.borrow().clone())
}

pub const PYTHON_EGRESS_GUARD_SITECUSTOMIZE: &str = r#"import ipaddress
import socket

_NAT64_PREFIX = ipaddress.ip_network("64:ff9b::/96")

def _blocked(ip):
    addr = ipaddress.ip_address(ip)
    if addr.version == 6:
        if addr.ipv4_mapped is not None:
            return _blocked(str(addr.ipv4_mapped))
        if addr in _NAT64_PREFIX:
            return _blocked(str(ipaddress.IPv4Address(int(addr) & 0xffffffff)))
    if addr.is_private or addr.is_loopback or addr.is_link_local \
       or addr.is_reserved or addr.is_multicast or addr.is_unspecified:
        return True
    if addr.version == 4 and addr in ipaddress.ip_network("100.64.0.0/10"):
        return True
    return False

_real_getaddrinfo = socket.getaddrinfo

def _guarded_getaddrinfo(host, *args, **kwargs):
    results = _real_getaddrinfo(host, *args, **kwargs)
    for result in results:
        ip = result[4][0]
        if _blocked(ip):
            raise OSError(
                "agentflow egress blocked: non-public address %s for host %s" % (ip, host)
            )
    return results

socket.getaddrinfo = _guarded_getaddrinfo

_real_connect = socket.socket.connect

def _guarded_connect(self, address):
    try:
        host = address[0]
        ipaddress.ip_address(host)
        if _blocked(host):
            raise OSError("agentflow egress blocked: non-public address %s" % host)
    except ValueError:
        pass
    return _real_connect(self, address)

socket.socket.connect = _guarded_connect
"#;

const SYNTH_TOOL_NAMESPACE: &str = "synth";
const MAX_RUNTIME_CAPTURE_BYTES: usize = 1024 * 1024;
const MAX_RUNTIME_ARTIFACT_TEXT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RUNTIME_ENVIRONMENT_YAML_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RUNTIME_LOG_READ_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowRunSummary {
    pub flow_id: String,
    pub completed_steps: usize,
    pub failed_steps: usize,
    pub attempts: Vec<AttemptSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptSummary {
    pub run_id: String,
    pub attempt_id: String,
    pub step_id: String,
    pub status: String,
    pub workdir: PathBuf,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheExplanation {
    pub flow_id: String,
    pub step_id: String,
    pub cache_key: String,
    pub hit: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntrySummary {
    pub cache_key: String,
    pub tool_ref: String,
    pub output_count: usize,
    pub created_at: i64,
    pub last_used_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachePruneSummary {
    pub removed_entries: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentCheckSummary {
    pub tool_ref: String,
    pub version: String,
    pub backend: String,
    pub ok: bool,
    pub items: Vec<EnvironmentCheckItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentCheckItem {
    pub name: String,
    pub status: String,
    pub message: String,
    pub details: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentPrepareSummary {
    pub tool_ref: String,
    pub version: String,
    pub backend: String,
    pub ok: bool,
    pub status: String,
    pub command: Vec<String>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub items: Vec<EnvironmentCheckItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentExportSummary {
    pub tool_ref: String,
    pub version: String,
    pub backend: String,
    pub ok: bool,
    pub status: String,
    pub command: Vec<String>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub export_hash: Option<String>,
    pub declared_packages: Vec<String>,
    pub exported_packages: Vec<String>,
    pub missing_packages: Vec<String>,
    pub extra_packages: Vec<String>,
    pub items: Vec<EnvironmentCheckItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsolatedEnvState {
    pub prefix: PathBuf,
    pub lock_hash: String,
}

pub trait IsolatedEnvProvisioner {
    fn ensure(&self, env_file: &Path, prefix: &Path, runner: &str) -> Result<(), StorageError>;
}

pub struct MicromambaProvisioner;

impl IsolatedEnvProvisioner for MicromambaProvisioner {
    fn ensure(&self, env_file: &Path, prefix: &Path, runner: &str) -> Result<(), StorageError> {
        let parent = prefix.parent().ok_or_else(|| {
            StorageError::InvalidInput(format!(
                "isolated environment prefix has no parent: {}",
                prefix.display()
            ))
        })?;
        fs::create_dir_all(parent)?;

        let mut create = Command::new(runner);
        create
            .args(["create", "-y", "-p"])
            .arg(prefix)
            .arg("-f")
            .arg(env_file)
            .env_clear()
            .env("PATH", "/usr/bin:/bin");
        let create_output = run_local_command(create, None).map_err(StorageError::Io)?;
        if !create_output.status.success() {
            return Err(provisioner_command_error("create", &create_output));
        }

        fs::create_dir_all(prefix)?;
        let mut export = Command::new(runner);
        export
            .args(["env", "export", "-p"])
            .arg(prefix)
            .arg("--explicit")
            .env_clear()
            .env("PATH", "/usr/bin:/bin");
        let export_output = run_local_command(export, None).map_err(StorageError::Io)?;
        if !export_output.status.success() {
            return Err(provisioner_command_error("export", &export_output));
        }

        let mut lock = fs::File::create(prefix.join(ISOLATED_ENV_LOCK_FILE))?;
        lock.write_all(&export_output.stdout)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLogs {
    pub attempt_id: String,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecordSummary {
    pub run_id: String,
    pub flow_id: String,
    pub step_id: String,
    pub status: String,
    pub attempt_count: i64,
    pub latest_attempt_id: Option<String>,
    pub cache_key: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunAttemptRecord {
    pub attempt_id: String,
    pub run_id: String,
    pub attempt: i64,
    pub status: String,
    pub workdir: Option<PathBuf>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub exit_code: Option<i32>,
    pub stdout_path: Option<PathBuf>,
    pub stderr_path: Option<PathBuf>,
    pub error_class: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunInspection {
    pub run: RunRecordSummary,
    pub attempts: Vec<RunAttemptRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RuntimeStatusJson {
    schema_version: String,
    project: RuntimeStatusProjectJson,
    counts: RuntimeStatusCountsJson,
}

#[derive(Debug, Serialize, Deserialize)]
struct RuntimeStatusProjectJson {
    id: String,
    name: String,
    root_path: String,
    engine_version: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct RuntimeStatusCountsJson {
    flows: i64,
    steps: i64,
    runs: i64,
    run_attempts: i64,
    artifacts: i64,
}

impl ProjectStore {
    pub fn check_tool_environment(
        &self,
        tool_ref: &str,
    ) -> Result<EnvironmentCheckSummary, StorageError> {
        let tool = self.executable_tool(tool_ref)?;
        let mut items = Vec::new();
        match tool.runtime.backend.as_str() {
            "local" => {
                items.push(EnvironmentCheckItem::ok(
                    "backend",
                    "local runtime does not require an external environment",
                    None,
                ));
            }
            "conda" | "micromamba" => {
                check_runner(&tool.runtime, &mut items);
                check_environment_selector(&tool.runtime, &mut items);
                check_env_file(&tool.runtime, &mut items);
                check_environment_probe(&tool.runtime, &mut items);
            }
            "isolated-micromamba" => {
                check_runner(&tool.runtime, &mut items);
                check_isolated_environment(
                    self.root_path(),
                    &tool.tool_ref,
                    &tool.runtime,
                    &mut items,
                );
            }
            other => items.push(EnvironmentCheckItem::failed(
                "backend",
                format!("unsupported runtime backend {other}"),
                None,
            )),
        }
        let ok = items.iter().all(|item| item.status == "ok");
        Ok(EnvironmentCheckSummary {
            tool_ref: tool.tool_ref,
            version: tool.version,
            backend: tool.runtime.backend,
            ok,
            items,
        })
    }

    pub fn prepare_tool_environment(
        &self,
        tool_ref: &str,
    ) -> Result<EnvironmentPrepareSummary, StorageError> {
        let tool = self.executable_tool(tool_ref)?;
        let mut items = Vec::new();
        let mut command = Vec::new();
        let mut exit_code = None;
        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut status = "failed".to_string();

        match tool.runtime.backend.as_str() {
            "local" => {
                items.push(EnvironmentCheckItem::failed(
                    "backend",
                    "local runtime does not support env prepare",
                    None,
                ));
            }
            "conda" | "micromamba" => {
                check_runner(&tool.runtime, &mut items);
                check_prepare_environment_selector(&tool.runtime, &mut items);
                check_required_env_file(&tool.runtime, &mut items);
                if items.iter().all(|item| item.status == "ok") {
                    let prepared = prepare_environment_update_command(&tool.runtime)?;
                    command = prepared.argv();
                    let mut process = Command::new(&prepared.executable);
                    process
                        .args(&prepared.args)
                        .env_clear()
                        .env("PATH", "/usr/bin:/bin");
                    match run_local_command(process, tool.runtime.timeout_seconds) {
                        Ok(output) => {
                            exit_code = output.status.code();
                            stdout = String::from_utf8_lossy(&output.stdout).to_string();
                            stderr = String::from_utf8_lossy(&output.stderr).to_string();
                            if output.timed_out {
                                status = "timed_out".to_string();
                                items.push(EnvironmentCheckItem::failed(
                                    "prepare",
                                    format!(
                                        "environment prepare timed out after {} seconds",
                                        output.timeout_seconds.unwrap_or_default()
                                    ),
                                    None,
                                ));
                            } else if output.status.success() {
                                status = "succeeded".to_string();
                                items.push(EnvironmentCheckItem::ok(
                                    "prepare",
                                    "environment prepare command succeeded",
                                    None,
                                ));
                            } else {
                                items.push(EnvironmentCheckItem::failed(
                                    "prepare",
                                    format!(
                                        "environment prepare exited with code {:?}",
                                        output.status.code()
                                    ),
                                    None,
                                ));
                            }
                        }
                        Err(error) => {
                            stderr = error.to_string();
                            items.push(EnvironmentCheckItem::failed(
                                "prepare",
                                "environment prepare command could not start",
                                Some(error.to_string()),
                            ));
                        }
                    }
                }
            }
            "isolated-micromamba" => {
                check_runner(&tool.runtime, &mut items);
                check_isolated_runtime_selector(&tool.runtime, &mut items);
                check_required_env_file(&tool.runtime, &mut items);
                if items.iter().all(|item| item.status == "ok") {
                    match isolated_env_state_for_tool(
                        self.root_path(),
                        &tool.tool_ref,
                        &tool.runtime,
                    ) {
                        Ok(Some(state)) => {
                            command =
                                isolated_environment_create_argv(&tool.runtime, &state.prefix)?;
                            match ensure_isolated_tool_environment(
                                self.root_path(),
                                &tool.tool_ref,
                                &tool.runtime,
                                &MicromambaProvisioner,
                            ) {
                                Ok(Some(ready)) => {
                                    status = "succeeded".to_string();
                                    items.push(EnvironmentCheckItem::ok(
                                        "prepare",
                                        "isolated environment is ready",
                                        Some(format!(
                                            "prefix={}; lock_hash={}",
                                            ready.prefix.display(),
                                            ready.lock_hash
                                        )),
                                    ));
                                }
                                Ok(None) => items.push(EnvironmentCheckItem::failed(
                                    "prepare",
                                    "isolated environment prepare returned no managed prefix",
                                    None,
                                )),
                                Err(error) => {
                                    stderr = error.to_string();
                                    items.push(EnvironmentCheckItem::failed(
                                        "prepare",
                                        "isolated environment prepare failed",
                                        Some(error.to_string()),
                                    ));
                                }
                            }
                        }
                        Ok(None) => items.push(EnvironmentCheckItem::failed(
                            "prepare",
                            "isolated environment state unavailable",
                            None,
                        )),
                        Err(error) => {
                            stderr = error.to_string();
                            items.push(EnvironmentCheckItem::failed(
                                "prepare",
                                "isolated environment metadata failed",
                                Some(error.to_string()),
                            ));
                        }
                    }
                }
            }
            other => items.push(EnvironmentCheckItem::failed(
                "backend",
                format!("unsupported runtime backend {other}"),
                None,
            )),
        }

        let ok = status == "succeeded" && items.iter().all(|item| item.status == "ok");
        Ok(EnvironmentPrepareSummary {
            tool_ref: tool.tool_ref,
            version: tool.version,
            backend: tool.runtime.backend,
            ok,
            status,
            command,
            exit_code,
            stdout,
            stderr,
            items,
        })
    }

    pub fn export_tool_environment(
        &self,
        tool_ref: &str,
    ) -> Result<EnvironmentExportSummary, StorageError> {
        let tool = self.executable_tool(tool_ref)?;
        let mut items = Vec::new();
        let mut command = Vec::new();
        let mut exit_code = None;
        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut status = "failed".to_string();
        let mut export_hash = None;
        let mut declared_packages = Vec::new();
        let mut exported_packages = Vec::new();
        let mut missing_packages = Vec::new();
        let mut extra_packages = Vec::new();

        match tool.runtime.backend.as_str() {
            "local" => {
                items.push(EnvironmentCheckItem::failed(
                    "backend",
                    "local runtime does not support env export",
                    None,
                ));
            }
            "conda" | "micromamba" => {
                check_runner(&tool.runtime, &mut items);
                check_environment_selector(&tool.runtime, &mut items);
                check_env_file(&tool.runtime, &mut items);
                if items.iter().all(|item| item.status == "ok") {
                    declared_packages = declared_environment_packages(&tool.runtime)?;
                    let prepared = prepare_environment_export_command(&tool.runtime)?;
                    command = prepared.argv();
                    let mut process = Command::new(&prepared.executable);
                    process
                        .args(&prepared.args)
                        .env_clear()
                        .env("PATH", "/usr/bin:/bin");
                    match run_local_command(process, tool.runtime.timeout_seconds) {
                        Ok(output) => {
                            exit_code = output.status.code();
                            stdout = String::from_utf8_lossy(&output.stdout).to_string();
                            stderr = String::from_utf8_lossy(&output.stderr).to_string();
                            if output.timed_out {
                                status = "timed_out".to_string();
                                items.push(EnvironmentCheckItem::failed(
                                    "export",
                                    format!(
                                        "environment export timed out after {} seconds",
                                        output.timeout_seconds.unwrap_or_default()
                                    ),
                                    None,
                                ));
                            } else if output.status.success() {
                                status = "succeeded".to_string();
                                export_hash = Some(stable_hash(&stdout));
                                exported_packages = extract_conda_dependency_packages(&stdout);
                                let diff =
                                    dependency_package_diff(&declared_packages, &exported_packages);
                                missing_packages = diff.missing;
                                extra_packages = diff.extra;
                                items.push(EnvironmentCheckItem::ok(
                                    "export",
                                    "environment export command succeeded",
                                    None,
                                ));
                                items.push(environment_package_diff_item(
                                    declared_packages.len(),
                                    exported_packages.len(),
                                    &missing_packages,
                                    &extra_packages,
                                ));
                            } else {
                                items.push(EnvironmentCheckItem::failed(
                                    "export",
                                    format!(
                                        "environment export exited with code {:?}",
                                        output.status.code()
                                    ),
                                    None,
                                ));
                            }
                        }
                        Err(error) => {
                            stderr = error.to_string();
                            items.push(EnvironmentCheckItem::failed(
                                "export",
                                "environment export command could not start",
                                Some(error.to_string()),
                            ));
                        }
                    }
                }
            }
            other => items.push(EnvironmentCheckItem::failed(
                "backend",
                format!("unsupported runtime backend {other}"),
                None,
            )),
        }

        let ok = status == "succeeded" && items.iter().all(|item| item.status != "failed");
        Ok(EnvironmentExportSummary {
            tool_ref: tool.tool_ref,
            version: tool.version,
            backend: tool.runtime.backend,
            ok,
            status,
            command,
            exit_code,
            stdout,
            stderr,
            export_hash,
            declared_packages,
            exported_packages,
            missing_packages,
            extra_packages,
            items,
        })
    }

    pub fn run_flow(&self, flow_id: &str) -> Result<FlowRunSummary, StorageError> {
        self.run_flow_with(flow_id, &RunConfig::default())
    }

    pub fn run_flow_with(
        &self,
        flow_id: &str,
        config: &RunConfig,
    ) -> Result<FlowRunSummary, StorageError> {
        if self.inspect_flow(flow_id)?.status != "approved" {
            return Err(StorageError::InvalidInput(format!(
                "flow {flow_id} must be approved before run"
            )));
        }

        let mut completed_steps = 0;
        let mut failed_steps = 0;
        let mut attempts = Vec::new();
        // In keep-going mode a failed step is terminal: record its id so it is not
        // re-offered as "ready" (ready_steps re-includes failed steps for retry).
        let mut failed_ids: BTreeSet<String> = BTreeSet::new();

        loop {
            let flow = self.inspect_flow(flow_id)?;
            let mut completed = completed_step_ids(&flow.steps);
            let mut ready = RuleBasedStepScheduler.order(
                ready_steps(&flow.steps, &flow.edges, &completed),
                &flow.edges,
            );
            if config.keep_going {
                ready.retain(|step| !failed_ids.contains(&step.id));
            }
            if ready.is_empty() {
                break;
            }

            let mut progressed = false;
            if config.max_parallel > 1 {
                // Parallel wave: prepare + record stay serial on the main thread
                // (single connection); only the tool subprocesses overlap.
                for attempt in self.run_ready_wave_parallel(flow_id, &ready, config)? {
                    match attempt.status.as_str() {
                        "succeeded" | "cache_hit" => {
                            completed.insert(attempt.step_id.clone());
                            completed_steps += 1;
                            progressed = true;
                        }
                        _ => {
                            failed_steps += 1;
                            failed_ids.insert(attempt.step_id.clone());
                        }
                    }
                    attempts.push(attempt);
                }
            } else {
                for step in ready {
                    let attempt = self.run_step(flow_id, &step, config)?;
                    match attempt.status.as_str() {
                        "succeeded" | "cache_hit" => {
                            completed.insert(attempt.step_id.clone());
                            completed_steps += 1;
                            progressed = true;
                        }
                        _ => {
                            failed_steps += 1;
                            failed_ids.insert(attempt.step_id.clone());
                        }
                    }
                    attempts.push(attempt);
                    if !config.keep_going && failed_steps > 0 {
                        break;
                    }
                }
            }

            if !config.keep_going && failed_steps > 0 {
                break;
            }
            if !progressed {
                break;
            }
        }

        Ok(FlowRunSummary {
            flow_id: flow_id.to_string(),
            completed_steps,
            failed_steps,
            attempts,
        })
    }

    /// Run one scheduler wave with the tool subprocesses overlapping. Steps are
    /// taken in batches of at most `config.max_parallel` (guaranteed > 1 by the
    /// caller); each batch is prepared (DB reads + run/attempt inserts), its
    /// subprocesses run on `std::thread::scope` worker threads, then its results
    /// are recorded — preparation and recording stay on the main thread, so the
    /// single SQLite connection is never shared across threads. Like the
    /// sequential path, a batch that produces any failure stops the wave (later
    /// batches are neither prepared nor launched, so no run rows are orphaned).
    /// Returned in ready order, identical to a serial run of the same
    /// (independent) steps on the success path.
    fn run_ready_wave_parallel(
        &self,
        flow_id: &str,
        ready: &[StoredFlowStep],
        config: &RunConfig,
    ) -> Result<Vec<AttemptSummary>, StorageError> {
        let max = config.max_parallel;
        let mut attempts: Vec<AttemptSummary> = Vec::with_capacity(ready.len());
        let mut steps = ready.iter();
        loop {
            let batch: Vec<&StoredFlowStep> = steps.by_ref().take(max).collect();
            if batch.is_empty() {
                break;
            }

            // Prepare the batch (serial). Cache hits / pre-run validation failures
            // are already finished; the rest carry a built command.
            let mut batch_results: Vec<Option<AttemptSummary>> = Vec::with_capacity(batch.len());
            let mut pending: Vec<(usize, PendingStep)> = Vec::new();
            for step in &batch {
                match self.prepare_step(flow_id, step, config)? {
                    PreparedStep::Finished(summary) => batch_results.push(Some(summary)),
                    PreparedStep::Pending(prepared) => {
                        batch_results.push(None);
                        pending.push((batch_results.len() - 1, *prepared));
                    }
                }
            }

            // Run the pending subprocesses concurrently — no DB access on threads.
            let mut records: Vec<(usize, PendingStep)> = Vec::with_capacity(pending.len());
            let mut jobs: Vec<(usize, Command, Option<u64>)> = Vec::with_capacity(pending.len());
            for (slot, mut prepared) in pending {
                let command = prepared
                    .command
                    .take()
                    .expect("pending step retains its command until execution");
                let timeout = prepared.timeout_seconds;
                jobs.push((slot, command, timeout));
                records.push((slot, prepared));
            }
            let outputs: Vec<(usize, std::io::Result<LocalCommandOutput>)> =
                std::thread::scope(|scope| {
                    jobs.into_iter()
                        .map(|(slot, command, timeout)| {
                            scope.spawn(move || (slot, run_local_command(command, timeout)))
                        })
                        .collect::<Vec<_>>()
                        .into_iter()
                        .map(|handle| handle.join().expect("step subprocess thread panicked"))
                        .collect()
                });

            // Record results (serial).
            for (slot, output) in outputs {
                let position = records
                    .iter()
                    .position(|(candidate, _)| *candidate == slot)
                    .expect("recorded slot for executed subprocess");
                let (slot, prepared) = records.remove(position);
                batch_results[slot] = Some(self.record_step(prepared, output)?);
            }

            // Collect in ready order; in fail-fast mode stop the wave after any
            // failure (matching the sequential path). In keep-going mode run every
            // batch — the caller skips terminally-failed steps' dependents.
            let mut batch_failed = false;
            for summary in batch_results.into_iter().flatten() {
                if !matches!(summary.status.as_str(), "succeeded" | "cache_hit") {
                    batch_failed = true;
                }
                attempts.push(summary);
            }
            if batch_failed && !config.keep_going {
                break;
            }
        }

        Ok(attempts)
    }

    pub fn list_cache_entries(&self) -> Result<Vec<CacheEntrySummary>, StorageError> {
        let mut stmt = self.connection().prepare(
            "SELECT cache_key, tool_ref, output_artifacts_json, created_at, last_used_at
             FROM cache_entries
             ORDER BY last_used_at DESC, cache_key ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let output_artifacts_json = row.get::<_, String>(2)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                output_artifacts_json,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;

        let mut entries = Vec::new();
        for row in rows {
            let (cache_key, tool_ref, output_artifacts_json, created_at, last_used_at) = row?;
            entries.push(CacheEntrySummary {
                cache_key,
                tool_ref,
                output_count: parse_json_map(&output_artifacts_json)?.len(),
                created_at,
                last_used_at,
            });
        }
        Ok(entries)
    }

    pub fn prune_cache_entries(
        &self,
        older_than_seconds: Option<u64>,
    ) -> Result<CachePruneSummary, StorageError> {
        let removed = if let Some(seconds) = older_than_seconds {
            let seconds = i64::try_from(seconds).unwrap_or(i64::MAX);
            let cutoff = crate::storage::now_unix_seconds().saturating_sub(seconds);
            self.connection().execute(
                "DELETE FROM cache_entries WHERE last_used_at < ?1",
                params![cutoff],
            )?
        } else {
            self.connection().execute("DELETE FROM cache_entries", [])?
        };
        self.touch_project()?;
        Ok(CachePruneSummary {
            removed_entries: removed,
        })
    }

    pub fn cache_explain_flow(&self, flow_id: &str) -> Result<Vec<CacheExplanation>, StorageError> {
        let flow = self.inspect_flow(flow_id)?;
        let mut explanations = Vec::new();
        for step in &flow.steps {
            if !matches!(
                step.status.as_str(),
                "draft" | "ready" | "failed" | "completed"
            ) {
                continue;
            }
            explanations.push(self.cache_explanation_for_step(flow_id, step)?);
        }
        Ok(explanations)
    }

    pub fn cache_explain_target(
        &self,
        target: &str,
    ) -> Result<Vec<CacheExplanation>, StorageError> {
        match self.cache_explain_flow(target) {
            Ok(explanations) => Ok(explanations),
            Err(StorageError::NotFound(_)) => self.cache_explain_step_ref(target),
            Err(error) => Err(error),
        }
    }

    pub fn cache_explain_step_ref(
        &self,
        step_ref: &str,
    ) -> Result<Vec<CacheExplanation>, StorageError> {
        let (flow_id, step_local_id) = self.resolve_step_ref(step_ref)?;
        let flow = self.inspect_flow(&flow_id)?;
        let step = flow
            .steps
            .iter()
            .find(|step| step.local_id == step_local_id || step.id == step_local_id)
            .ok_or_else(|| {
                StorageError::NotFound(format!("step {step_local_id} in flow {flow_id}"))
            })?;
        Ok(vec![self.cache_explanation_for_step(&flow_id, step)?])
    }

    pub fn retry_step(
        &self,
        flow_id: &str,
        step_local_id: &str,
    ) -> Result<FlowRunSummary, StorageError> {
        let flow = self.inspect_flow(flow_id)?;
        if flow.status != "approved" {
            return Err(StorageError::InvalidInput(format!(
                "flow {flow_id} must be approved before retry"
            )));
        }
        let step = flow
            .steps
            .iter()
            .find(|step| step.local_id == step_local_id || step.id == step_local_id)
            .ok_or_else(|| {
                StorageError::NotFound(format!("step {step_local_id} in flow {flow_id}"))
            })?;
        if step.status != StepStatus::Failed.as_str() {
            return Err(StorageError::InvalidInput(format!(
                "retry currently supports failed steps only; step {} is {}",
                step.local_id, step.status
            )));
        }
        self.update_step_status(&step.id, StepStatus::Ready)?;
        self.run_flow(flow_id)
    }

    pub fn retry_step_ref(&self, step_ref: &str) -> Result<FlowRunSummary, StorageError> {
        let (flow_id, step_local_id) = self.resolve_step_ref(step_ref)?;
        self.retry_step(&flow_id, &step_local_id)
    }

    pub fn run_step_ref(&self, step_ref: &str) -> Result<FlowRunSummary, StorageError> {
        let scoped_config = scoped_run_config();
        let default_config;
        let config = if let Some(config) = scoped_config.as_ref() {
            config
        } else {
            default_config = RunConfig::default();
            &default_config
        };
        let (flow_id, step_local_id) = self.resolve_step_ref(step_ref)?;
        let flow = self.inspect_flow(&flow_id)?;
        if flow.status != "approved" {
            return Err(StorageError::InvalidInput(format!(
                "flow {flow_id} must be approved before run-step"
            )));
        }
        let step = flow
            .steps
            .iter()
            .find(|step| step.local_id == step_local_id || step.id == step_local_id)
            .ok_or_else(|| {
                StorageError::NotFound(format!("step {step_local_id} in flow {flow_id}"))
            })?;
        match step.status.as_str() {
            "draft" | "ready" | "failed" => {}
            other => {
                return Err(StorageError::InvalidInput(format!(
                    "run-step supports draft, ready, or failed steps only; step {} is {}",
                    step.local_id, other
                )));
            }
        }
        ensure_step_dependencies_completed(&flow.steps, &flow.edges, step)?;

        let attempt = self.run_step(&flow_id, step, config)?;
        let completed_steps =
            usize::from(matches!(attempt.status.as_str(), "succeeded" | "cache_hit"));
        let failed_steps = usize::from(completed_steps == 0);
        Ok(FlowRunSummary {
            flow_id,
            completed_steps,
            failed_steps,
            attempts: vec![attempt],
        })
    }

    pub fn read_logs(&self, id: &str) -> Result<RunLogs, StorageError> {
        let attempt = self
            .connection()
            .query_row(
                "SELECT id, stdout_path, stderr_path FROM run_attempts WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        PathBuf::from(row.get::<_, String>(1)?),
                        PathBuf::from(row.get::<_, String>(2)?),
                    ))
                },
            )
            .optional()?;
        let (attempt_id, stdout_path, stderr_path) = if let Some(attempt) = attempt {
            attempt
        } else {
            self.connection()
                .query_row(
                    "SELECT id, stdout_path, stderr_path
                     FROM run_attempts
                     WHERE run_id = ?1
                     ORDER BY attempt DESC
                     LIMIT 1",
                    params![id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            PathBuf::from(row.get::<_, String>(1)?),
                            PathBuf::from(row.get::<_, String>(2)?),
                        ))
                    },
                )
                .optional()?
                .ok_or_else(|| StorageError::NotFound(format!("run or attempt {id}")))?
        };

        Ok(RunLogs {
            attempt_id,
            stdout: read_text_file_with_byte_cap(
                "stdout log",
                &stdout_path,
                MAX_RUNTIME_LOG_READ_BYTES,
            )?,
            stderr: read_text_file_with_byte_cap(
                "stderr log",
                &stderr_path,
                MAX_RUNTIME_LOG_READ_BYTES,
            )?,
            stdout_path,
            stderr_path,
        })
    }

    pub fn list_runs(&self, flow_id: Option<&str>) -> Result<Vec<RunRecordSummary>, StorageError> {
        let sql = "SELECT id, flow_id, step_id, status, attempt_count, latest_attempt_id, cache_key, created_at, updated_at
                   FROM runs";
        let ordered_sql = if flow_id.is_some() {
            format!("{sql} WHERE flow_id = ?1 ORDER BY updated_at DESC, id ASC")
        } else {
            format!("{sql} ORDER BY updated_at DESC, id ASC")
        };
        let mut stmt = self.connection().prepare(&ordered_sql)?;
        let mut rows = if let Some(flow_id) = flow_id {
            stmt.query(params![flow_id])?
        } else {
            stmt.query([])?
        };

        let mut runs = Vec::new();
        while let Some(row) = rows.next()? {
            runs.push(run_record_from_row(row)?);
        }
        Ok(runs)
    }

    pub fn inspect_run_or_attempt(&self, id: &str) -> Result<RunInspection, StorageError> {
        let run_id = self
            .connection()
            .query_row("SELECT id FROM runs WHERE id = ?1", params![id], |row| {
                row.get::<_, String>(0)
            })
            .optional()?
            .map(Ok)
            .unwrap_or_else(|| {
                self.connection()
                    .query_row(
                        "SELECT run_id FROM run_attempts WHERE id = ?1",
                        params![id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?
                    .ok_or_else(|| StorageError::NotFound(format!("run or attempt {id}")))
            })?;

        let run = self.connection().query_row(
            "SELECT id, flow_id, step_id, status, attempt_count, latest_attempt_id, cache_key, created_at, updated_at
             FROM runs
             WHERE id = ?1",
            params![&run_id],
            run_record_from_row,
        )?;
        let attempts = self.run_attempt_records(&run_id)?;
        Ok(RunInspection { run, attempts })
    }

    fn run_attempt_records(&self, run_id: &str) -> Result<Vec<RunAttemptRecord>, StorageError> {
        let mut stmt = self.connection().prepare(
            "SELECT id, run_id, attempt, status, workdir, started_at, ended_at, exit_code, stdout_path, stderr_path, error_class, error_message
             FROM run_attempts
             WHERE run_id = ?1
             ORDER BY attempt ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![run_id], |row| {
            Ok(RunAttemptRecord {
                attempt_id: row.get::<_, String>(0)?,
                run_id: row.get::<_, String>(1)?,
                attempt: row.get::<_, i64>(2)?,
                status: row.get::<_, String>(3)?,
                workdir: row.get::<_, Option<String>>(4)?.map(PathBuf::from),
                started_at: row.get::<_, Option<i64>>(5)?,
                ended_at: row.get::<_, Option<i64>>(6)?,
                exit_code: row.get::<_, Option<i32>>(7)?,
                stdout_path: row.get::<_, Option<String>>(8)?.map(PathBuf::from),
                stderr_path: row.get::<_, Option<String>>(9)?.map(PathBuf::from),
                error_class: row.get::<_, Option<String>>(10)?,
                error_message: row.get::<_, Option<String>>(11)?,
            })
        })?;

        let mut attempts = Vec::new();
        for row in rows {
            attempts.push(row?);
        }
        Ok(attempts)
    }

    pub fn status_json(&self) -> Result<String, StorageError> {
        let summary = self.summary()?;
        let (flow_count, step_count, run_count, attempt_count, artifact_count) =
            self.connection().query_row(
                "SELECT
                   (SELECT COUNT(*) FROM flows),
                   (SELECT COUNT(*) FROM steps),
                   (SELECT COUNT(*) FROM runs),
                   (SELECT COUNT(*) FROM run_attempts),
                   (SELECT COUNT(*) FROM artifacts)",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )?;

        serde_json::to_string(&RuntimeStatusJson {
            schema_version: agentflow_schemas::STATUS_JSON_SCHEMA_V0.to_string(),
            project: RuntimeStatusProjectJson {
                id: summary.id,
                name: summary.name,
                root_path: summary.root_path.display().to_string(),
                engine_version: summary.engine_version,
                created_at: summary.created_at,
                updated_at: summary.updated_at,
            },
            counts: RuntimeStatusCountsJson {
                flows: flow_count,
                steps: step_count,
                runs: run_count,
                run_attempts: attempt_count,
                artifacts: artifact_count,
            },
        })
        .map_err(|err| StorageError::InvalidInput(format!("status JSON failed: {err}")))
    }

    fn run_step(
        &self,
        flow_id: &str,
        step: &StoredFlowStep,
        config: &RunConfig,
    ) -> Result<AttemptSummary, StorageError> {
        match self.prepare_step(flow_id, step, config)? {
            PreparedStep::Finished(summary) => Ok(summary),
            PreparedStep::Pending(pending) => self.execute_and_record_step(*pending),
        }
    }

    fn execute_and_record_step(
        &self,
        mut pending: PendingStep,
    ) -> Result<AttemptSummary, StorageError> {
        let command = pending
            .command
            .take()
            .expect("prepared step retains its command until execution");
        let output = run_local_command(command, pending.timeout_seconds);
        self.record_step(pending, output)
    }

    fn prepare_step(
        &self,
        flow_id: &str,
        step: &StoredFlowStep,
        config: &RunConfig,
    ) -> Result<PreparedStep, StorageError> {
        let tool_ref = step.tool_ref.as_deref().ok_or_else(|| {
            StorageError::InvalidInput(format!("step {} has no tool_ref", step.id))
        })?;
        let tool = self.executable_tool(tool_ref)?;
        let inputs = parse_json_map(&step.inputs_json)?;
        let params_map = parse_json_map(&step.params_json)?;
        let outputs = parse_json_map(&step.outputs_json)?;
        let resolved_inputs = self.resolve_inputs(flow_id, &inputs)?;
        let params_json = string_map_json(&params_map);
        let input_hashes_json = input_hashes_json(&resolved_inputs);
        let isolated_env = ensure_isolated_tool_environment(
            self.root_path(),
            &tool.tool_ref,
            &tool.runtime,
            &MicromambaProvisioner,
        )?;
        let runtime_config =
            runtime_config_json_for_tool(self.root_path(), &tool.tool_ref, &tool.runtime)?;
        let runtime_hash = stable_hash(&runtime_config);
        let params_hash = stable_hash(&params_json);
        let cache_key = compute_cache_key(
            tool_ref,
            &tool.version,
            &input_hashes_json,
            &params_hash,
            &runtime_hash,
        );
        let run_id = format!("run_{}", now_unix_nanos());
        let attempt_id = format!("attempt_{}", now_unix_nanos());
        let workdir = project_dir(self.root_path()).join("work").join(&attempt_id);
        fs::create_dir_all(&workdir)?;
        let workdir = fs::canonicalize(&workdir)?;
        // Container backends mount only the workdir; staged inputs must be real
        // copies, not symlinks into the (unmounted) artifact store.
        let stage_inputs_by_copy = tool.runtime.backend == "container";
        let resolved_input_paths = input_paths(&resolved_inputs, &workdir, stage_inputs_by_copy)?;
        let stdout_path = workdir.join("stdout.log");
        let stderr_path = workdir.join("stderr.log");
        let resolved_outputs = output_paths(&workdir, &outputs);
        fs::create_dir_all(resolved_outputs.root())?;
        let step_env_vars = env_vars(
            &resolved_input_paths,
            &params_map,
            resolved_outputs.as_map(),
        );
        let fixed_agentflow_env_vars = vec![
            (
                "AGENTFLOW_WORKDIR".to_string(),
                workdir.display().to_string(),
            ),
            (
                "AGENTFLOW_INPUTS_JSON".to_string(),
                workdir.join("inputs.json").display().to_string(),
            ),
            (
                "AGENTFLOW_PARAMS_JSON".to_string(),
                workdir.join("params.json").display().to_string(),
            ),
            (
                "AGENTFLOW_OUTPUTS_JSON".to_string(),
                workdir.join("outputs.json").display().to_string(),
            ),
        ];
        let agentflow_env_names = agentflow_env_names(&step_env_vars);
        let exec_ctx = backend::ExecContext {
            workdir: &workdir,
            staged_inputs: &resolved_input_paths,
            output_dir: resolved_outputs.root(),
            env_names: &agentflow_env_names,
            container_engine: config.container_engine.as_ref(),
        };

        let mut prepared_command =
            prepare_runtime_command_for_tool(&tool.runtime, isolated_env.as_ref(), &exec_ctx)?;
        let synth_pythonpath = if tool.namespace == SYNTH_TOOL_NAMESPACE {
            let guard_dir = install_runtime_python_egress_guard(&workdir)?;
            // Cooperative defense-in-depth for generated synth tools. This is
            // not an anti-tamper sandbox; deployment-level egress containment
            // remains the hard boundary.
            prepend_python_egress_guard_to_runtime_command(&mut prepared_command, &guard_dir)?
        } else {
            None
        };
        materialize_workdir(
            &workdir,
            &prepared_command.argv(),
            &runtime_config,
            &resolved_input_paths,
            &params_map,
            resolved_outputs.as_map(),
        )?;

        let now = crate::storage::now_unix_seconds();
        self.connection().execute(
            "INSERT INTO runs
             (id, flow_id, step_id, status, attempt_count, latest_attempt_id, cache_key, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'running', 1, ?4, ?5, ?6, ?7)",
            params![
                &run_id,
                flow_id,
                &step.id,
                &attempt_id,
                &cache_key,
                now,
                now
            ],
        )?;
        self.connection().execute(
            "INSERT INTO run_attempts
             (id, run_id, attempt, status, workdir, started_at, stdout_path, stderr_path)
             VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7)",
            params![
                &attempt_id,
                &run_id,
                RunAttemptStatus::Running.as_str(),
                workdir.display().to_string(),
                now,
                stdout_path.display().to_string(),
                stderr_path.display().to_string()
            ],
        )?;

        self.update_step_status(&step.id, StepStatus::Ready)?;
        self.update_step_status(&step.id, StepStatus::Running)?;

        if let Err(error) = validate_declared_inputs(&resolved_inputs, &tool.inputs) {
            fs::write(&stdout_path, "")?;
            fs::write(&stderr_path, error.to_string())?;
            return Ok(PreparedStep::Finished(self.finish_attempt(
                FinishAttempt {
                    run_id,
                    attempt_id,
                    step_id: step.id.clone(),
                    workdir,
                    stdout_path,
                    stderr_path,
                    status: RunAttemptStatus::Failed,
                    exit_code: None,
                    error_message: Some(error.to_string()),
                },
            )?));
        }

        if let Some(cached_outputs) = self.cache_entry(&cache_key)? {
            let restore_result = self.restore_cached_outputs(
                &cached_outputs,
                &step.id,
                &run_id,
                &resolved_outputs,
                &tool.outputs,
            );
            let (status, error_message) = match restore_result {
                Ok(()) => {
                    fs::write(&stdout_path, format!("cache hit: {cache_key}\n"))?;
                    fs::write(&stderr_path, "")?;
                    self.touch_cache_entry(&cache_key)?;
                    (RunAttemptStatus::CacheHit, None)
                }
                Err(error) => {
                    fs::write(&stdout_path, "")?;
                    fs::write(&stderr_path, error.to_string())?;
                    (RunAttemptStatus::Failed, Some(error.to_string()))
                }
            };
            return Ok(PreparedStep::Finished(self.finish_attempt(
                FinishAttempt {
                    run_id,
                    attempt_id,
                    step_id: step.id.clone(),
                    workdir,
                    stdout_path,
                    stderr_path,
                    status,
                    exit_code: None,
                    error_message,
                },
            )?));
        }

        let mut command = Command::new(&prepared_command.executable);
        command
            .args(&prepared_command.args)
            .current_dir(&workdir)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("AGENTFLOW_WORKDIR", &workdir)
            .env("AGENTFLOW_INPUTS_JSON", workdir.join("inputs.json"))
            .env("AGENTFLOW_PARAMS_JSON", workdir.join("params.json"))
            .env("AGENTFLOW_OUTPUTS_JSON", workdir.join("outputs.json"))
            .envs(step_env_vars.iter().map(|(name, value)| (name, value)))
            .envs(singularity_env_vars(
                config,
                &fixed_agentflow_env_vars,
                &step_env_vars,
            ));
        if let Some(pythonpath) = synth_pythonpath {
            command.env("PYTHONPATH", pythonpath);
        }
        Ok(PreparedStep::Pending(Box::new(PendingStep {
            command: Some(command),
            timeout_seconds: tool.runtime.timeout_seconds,
            tool_ref: tool_ref.to_string(),
            tool_outputs: tool.outputs.clone(),
            run_id,
            attempt_id,
            step_id: step.id.clone(),
            workdir,
            stdout_path,
            stderr_path,
            resolved_outputs,
            cache_key,
            input_hashes_json,
            params_hash,
            runtime_hash,
        })))
    }

    fn record_step(
        &self,
        pending: PendingStep,
        output: std::io::Result<LocalCommandOutput>,
    ) -> Result<AttemptSummary, StorageError> {
        let PendingStep {
            command: _,
            timeout_seconds: _,
            tool_ref,
            tool_outputs,
            run_id,
            attempt_id,
            step_id,
            workdir,
            stdout_path,
            stderr_path,
            resolved_outputs,
            cache_key,
            input_hashes_json,
            params_hash,
            runtime_hash,
        } = pending;

        let (status, exit_code, error_message) = match output {
            Ok(output) => {
                let code = output.status.code();
                if let Err(error) = fs::write(&stdout_path, output.stdout)
                    .and_then(|_| fs::write(&stderr_path, output.stderr))
                {
                    (RunAttemptStatus::Failed, code, Some(error.to_string()))
                } else if output.timed_out {
                    let message = format!(
                        "command timed out after {} seconds",
                        output.timeout_seconds.unwrap_or_default()
                    );
                    let message =
                        write_runtime_error(&stderr_path, &StorageError::InvalidInput(message));
                    (RunAttemptStatus::TimedOut, code, Some(message))
                } else if output.status.success() {
                    match validate_outputs(&resolved_outputs) {
                        Ok(()) => match validate_declared_outputs(&resolved_outputs, &tool_outputs)
                        {
                            Ok(()) => {
                                let mut published_outputs = BTreeMap::new();
                                let publish_result = resolved_outputs.as_map().iter().try_for_each(
                                    |(output_name, output_path)| {
                                        let artifact_type = tool_outputs
                                            .get(output_name)
                                            .map(|port| port.type_name.clone())
                                            .unwrap_or_else(|| "File".to_string());
                                        let artifact = self.register_computed_artifact(
                                            ComputedArtifactRequest {
                                                source_path: output_path.clone(),
                                                artifact_type,
                                                output_name: output_name.clone(),
                                                source_step_id: step_id.clone(),
                                                source_run_id: run_id.clone(),
                                            },
                                        )?;
                                        self.observe_declared_output(
                                            &artifact.summary.id,
                                            output_name,
                                            &tool_outputs,
                                        )?;
                                        published_outputs.insert(
                                            output_name.clone(),
                                            artifact.summary.id.clone(),
                                        );
                                        Ok::<(), StorageError>(())
                                    },
                                );
                                match publish_result {
                                    Ok(()) => {
                                        self.save_cache_entry(CacheEntryWrite {
                                            cache_key: cache_key.clone(),
                                            tool_ref: tool_ref.to_string(),
                                            input_hashes_json: input_hashes_json.clone(),
                                            params_hash: params_hash.clone(),
                                            runtime_hash: runtime_hash.clone(),
                                            output_artifacts_json: string_map_json(
                                                &published_outputs,
                                            ),
                                        })?;
                                        (RunAttemptStatus::Succeeded, code, None)
                                    }
                                    Err(error) => {
                                        let message = write_runtime_error(&stderr_path, &error);
                                        (RunAttemptStatus::Failed, code, Some(message))
                                    }
                                }
                            }
                            Err(error) => {
                                let message = write_runtime_error(&stderr_path, &error);
                                (RunAttemptStatus::Failed, code, Some(message))
                            }
                        },
                        Err(error) => {
                            let message = write_runtime_error(&stderr_path, &error);
                            (RunAttemptStatus::Failed, code, Some(message))
                        }
                    }
                } else {
                    (
                        RunAttemptStatus::Failed,
                        code,
                        Some(format!("command exited with code {:?}", code)),
                    )
                }
            }
            Err(error) => {
                let message = error.to_string();
                let _ = fs::write(&stdout_path, "");
                let _ = fs::write(&stderr_path, &message);
                (RunAttemptStatus::Failed, None, Some(message))
            }
        };

        self.finish_attempt(FinishAttempt {
            run_id,
            attempt_id,
            step_id: step_id.clone(),
            workdir,
            stdout_path,
            stderr_path,
            status,
            exit_code,
            error_message,
        })
    }

    fn finish_attempt(&self, attempt: FinishAttempt) -> Result<AttemptSummary, StorageError> {
        let ended = crate::storage::now_unix_seconds();
        self.connection().execute(
            "UPDATE run_attempts
             SET status = ?1, ended_at = ?2, exit_code = ?3, error_class = ?4, error_message = ?5
             WHERE id = ?6",
            params![
                attempt.status.as_str(),
                ended,
                attempt.exit_code,
                attempt.error_message.as_ref().map(|_| "runtime"),
                attempt.error_message,
                &attempt.attempt_id
            ],
        )?;

        let step_status = if matches!(
            attempt.status,
            RunAttemptStatus::Succeeded | RunAttemptStatus::CacheHit
        ) {
            StepStatus::Completed
        } else {
            StepStatus::Failed
        };
        self.connection().execute(
            "UPDATE runs SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![step_status.as_str(), ended, &attempt.run_id],
        )?;
        self.update_step_status(&attempt.step_id, step_status)?;

        Ok(AttemptSummary {
            run_id: attempt.run_id,
            attempt_id: attempt.attempt_id,
            step_id: attempt.step_id,
            status: attempt.status.as_str().to_string(),
            workdir: attempt.workdir,
            stdout_path: attempt.stdout_path,
            stderr_path: attempt.stderr_path,
            exit_code: attempt.exit_code,
        })
    }

    fn cache_key_for_step(
        &self,
        flow_id: &str,
        step: &StoredFlowStep,
    ) -> Result<String, StorageError> {
        let tool_ref = step.tool_ref.as_deref().ok_or_else(|| {
            StorageError::InvalidInput(format!("step {} has no tool_ref", step.id))
        })?;
        let tool = self.executable_tool(tool_ref)?;
        let inputs = parse_json_map(&step.inputs_json)?;
        let params_map = parse_json_map(&step.params_json)?;
        let resolved_inputs = self.resolve_inputs(flow_id, &inputs)?;
        let input_hashes_json = input_hashes_json(&resolved_inputs);
        let params_hash = stable_hash(&string_map_json(&params_map));
        let runtime_hash = stable_hash(&runtime_config_json_for_tool(
            self.root_path(),
            tool_ref,
            &tool.runtime,
        )?);
        Ok(compute_cache_key(
            tool_ref,
            &tool.version,
            &input_hashes_json,
            &params_hash,
            &runtime_hash,
        ))
    }

    fn cache_explanation_for_step(
        &self,
        flow_id: &str,
        step: &StoredFlowStep,
    ) -> Result<CacheExplanation, StorageError> {
        let cache_key = match self.cache_key_for_step(flow_id, step) {
            Ok(cache_key) => cache_key,
            Err(error) => {
                return Ok(CacheExplanation {
                    flow_id: flow_id.to_string(),
                    step_id: step.id.clone(),
                    cache_key: "unavailable".to_string(),
                    hit: false,
                    reason: format!("cache key unavailable: {error}"),
                });
            }
        };
        let hit = self.cache_entry(&cache_key)?.is_some();
        Ok(CacheExplanation {
            flow_id: flow_id.to_string(),
            step_id: step.id.clone(),
            cache_key,
            hit,
            reason: if hit {
                "matching cache entry exists".to_string()
            } else {
                "no matching cache entry".to_string()
            },
        })
    }

    fn cache_entry(
        &self,
        cache_key: &str,
    ) -> Result<Option<BTreeMap<String, String>>, StorageError> {
        self.connection()
            .query_row(
                "SELECT output_artifacts_json FROM cache_entries WHERE cache_key = ?1",
                params![cache_key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| parse_json_map(&json))
            .transpose()
    }

    fn touch_cache_entry(&self, cache_key: &str) -> Result<(), StorageError> {
        self.connection().execute(
            "UPDATE cache_entries SET last_used_at = ?1 WHERE cache_key = ?2",
            params![crate::storage::now_unix_seconds(), cache_key],
        )?;
        Ok(())
    }

    fn save_cache_entry(&self, entry: CacheEntryWrite) -> Result<(), StorageError> {
        let now = crate::storage::now_unix_seconds();
        self.connection().execute(
            "INSERT INTO cache_entries
             (cache_key, tool_ref, input_hashes_json, params_hash, runtime_hash, output_artifacts_json, created_at, last_used_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(cache_key) DO UPDATE SET
               output_artifacts_json = excluded.output_artifacts_json,
               last_used_at = excluded.last_used_at",
            params![
                entry.cache_key,
                entry.tool_ref,
                entry.input_hashes_json,
                entry.params_hash,
                entry.runtime_hash,
                entry.output_artifacts_json,
                now,
                now
            ],
        )?;
        Ok(())
    }

    fn restore_cached_outputs(
        &self,
        cached_outputs: &BTreeMap<String, String>,
        step_id: &str,
        run_id: &str,
        resolved_outputs: &OutputPaths,
        tool_outputs: &BTreeMap<String, crate::storage::ToolPortSpec>,
    ) -> Result<(), StorageError> {
        for output_name in resolved_outputs.as_map().keys() {
            let artifact_id = cached_outputs.get(output_name).ok_or_else(|| {
                StorageError::InvalidInput(format!(
                    "cache entry is missing output artifact for {output_name}"
                ))
            })?;
            let artifact = self.inspect_artifact(artifact_id)?;
            let artifact_type = tool_outputs
                .get(output_name)
                .map(|port| port.type_name.clone())
                .unwrap_or_else(|| artifact.summary.artifact_type.clone());
            if let Some(port) = tool_outputs.get(output_name) {
                validate_port_file("output", output_name, &artifact.summary.path, port)?;
            }
            self.register_computed_artifact(ComputedArtifactRequest {
                source_path: artifact.summary.path,
                artifact_type,
                output_name: output_name.clone(),
                source_step_id: step_id.to_string(),
                source_run_id: run_id.to_string(),
            })
            .and_then(|artifact| {
                self.observe_declared_output(&artifact.summary.id, output_name, tool_outputs)
            })?;
        }
        Ok(())
    }

    fn observe_declared_output(
        &self,
        artifact_id: &str,
        output_name: &str,
        tool_outputs: &BTreeMap<String, crate::storage::ToolPortSpec>,
    ) -> Result<(), StorageError> {
        let Some(observer) = tool_outputs
            .get(output_name)
            .and_then(|port| port.observer.as_deref())
        else {
            return Ok(());
        };
        self.observe_artifact_with_adapter(artifact_id, observer)?;
        Ok(())
    }

    fn update_step_status(&self, step_id: &str, next: StepStatus) -> Result<(), StorageError> {
        let current: String = self.connection().query_row(
            "SELECT status FROM steps WHERE id = ?1",
            params![step_id],
            |row| row.get(0),
        )?;
        let current = StepStatus::parse(&current).ok_or_else(|| {
            StorageError::InvalidInput(format!("unknown current step status {current}"))
        })?;
        if current == next {
            return Ok(());
        }
        if !current.can_transition_to(next) {
            return Err(StorageError::InvalidInput(format!(
                "illegal step status transition {current} -> {next}"
            )));
        }
        self.connection().execute(
            "UPDATE steps SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![next.as_str(), crate::storage::now_unix_seconds(), step_id],
        )?;
        Ok(())
    }

    fn resolve_inputs(
        &self,
        flow_id: &str,
        inputs: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, ResolvedInput>, StorageError> {
        let mut resolved = BTreeMap::new();
        for (name, value) in inputs {
            if let Some((producer_step, output_name)) = value.split_once('.') {
                let artifact = self.resolve_step_output(flow_id, producer_step, output_name)?;
                resolved.insert(
                    name.clone(),
                    ResolvedInput {
                        path: artifact.path,
                        cache_identity: artifact.cache_identity,
                    },
                );
                continue;
            }
            let artifact_id = value.strip_prefix("artifact:").unwrap_or(value);
            if artifact_id.starts_with("artifact_") {
                let artifact = self.inspect_artifact(artifact_id)?;
                resolved.insert(
                    name.clone(),
                    ResolvedInput {
                        cache_identity: file_hash_fnv64(&artifact.summary.path)?,
                        path: artifact.summary.path,
                    },
                );
                continue;
            }
            return Err(StorageError::InvalidInput(format!(
                "runtime input {name} must reference artifact:<id>, artifact_<id>, or step.output; got {value}"
            )));
        }
        Ok(resolved)
    }

    fn resolve_step_output(
        &self,
        flow_id: &str,
        producer_step: &str,
        output_name: &str,
    ) -> Result<ResolvedStepOutput, StorageError> {
        let producer_step_id = format!("step:{flow_id}/{producer_step}");
        let mut stmt = self.connection().prepare(
            "SELECT path, validation_json
                 FROM artifacts
                 WHERE kind = 'computed'
                   AND source_step_id = ?1
                 ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map(params![producer_step_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (path, validation_json) = row?;
            if validation_output_name(&validation_json).as_deref() == Some(output_name) {
                let artifact = self.inspect_artifact_by_path(&path)?;
                return Ok(ResolvedStepOutput {
                    cache_identity: file_hash_fnv64(&artifact.path)?,
                    path: PathBuf::from(path),
                });
            }
        }
        Err(StorageError::NotFound(format!(
            "computed output {producer_step}.{output_name} for flow {flow_id}"
        )))
    }

    fn inspect_artifact_by_path(
        &self,
        path: &str,
    ) -> Result<crate::storage::ArtifactSummary, StorageError> {
        self.connection()
            .query_row(
                "SELECT id, kind, type, path, hash, size_bytes, source_step_id, source_run_id, created_at
                 FROM artifacts
                 WHERE path = ?1
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
                params![path],
                |row| {
                    Ok(crate::storage::ArtifactSummary {
                        id: row.get(0)?,
                        kind: row.get(1)?,
                        artifact_type: row.get(2)?,
                        path: PathBuf::from(row.get::<_, String>(3)?),
                        hash: row.get(4)?,
                        size_bytes: row.get(5)?,
                        source_step_id: row.get(6)?,
                        source_run_id: row.get(7)?,
                        created_at: row.get(8)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(format!("artifact path {path}")))
    }

    fn resolve_step_ref(&self, step_ref: &str) -> Result<(String, String), StorageError> {
        if let Some((flow_id, step_local_id)) = parse_step_ref(step_ref) {
            return Ok((flow_id, step_local_id));
        }

        let mut stmt = self.connection().prepare(
            "SELECT flow_id, id
             FROM steps",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut matches = Vec::new();
        for row in rows {
            let (flow_id, step_id) = row?;
            if local_step_id(&step_id) == step_ref {
                matches.push((flow_id, step_id));
            }
        }
        match matches.as_slice() {
            [(flow_id, step_id)] => Ok((flow_id.clone(), local_step_id(step_id))),
            [] => Err(StorageError::NotFound(format!("step {step_ref}"))),
            _ => Err(StorageError::InvalidInput(format!(
                "step ref {step_ref} is ambiguous; use flow.step or step:flow/step"
            ))),
        }
    }
}

fn ready_steps(
    steps: &[StoredFlowStep],
    edges: &[crate::storage::StoredFlowEdge],
    completed: &BTreeSet<String>,
) -> Vec<StoredFlowStep> {
    let mut ready = Vec::new();
    for step in steps {
        if !matches!(step.status.as_str(), "draft" | "ready" | "failed") {
            continue;
        }
        let needs = edges
            .iter()
            .filter(|edge| edge.to_step_id == step.id)
            .map(|edge| edge.from_step_id.as_str());
        if needs.clone().all(|need| completed.contains(need)) {
            ready.push(step.clone());
        }
    }
    ready
}

fn ensure_step_dependencies_completed(
    steps: &[StoredFlowStep],
    edges: &[crate::storage::StoredFlowEdge],
    step: &StoredFlowStep,
) -> Result<(), StorageError> {
    let completed = completed_step_ids(steps);
    let missing = edges
        .iter()
        .filter(|edge| edge.to_step_id == step.id && !completed.contains(&edge.from_step_id))
        .map(|edge| edge.from_local_id.clone())
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(StorageError::InvalidInput(format!(
            "run-step cannot execute {} before dependencies complete: {}",
            step.local_id,
            missing.join(", ")
        )))
    }
}

#[derive(Debug, Deserialize)]
struct ArtifactValidationOutputName {
    #[serde(default)]
    output_name: Option<String>,
}

fn validation_output_name(validation_json: &str) -> Option<String> {
    serde_json::from_str::<ArtifactValidationOutputName>(validation_json)
        .ok()
        .and_then(|payload| payload.output_name)
}

fn run_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRecordSummary> {
    Ok(RunRecordSummary {
        run_id: row.get::<_, String>(0)?,
        flow_id: row.get::<_, String>(1)?,
        step_id: row.get::<_, String>(2)?,
        status: row.get::<_, String>(3)?,
        attempt_count: row.get::<_, i64>(4)?,
        latest_attempt_id: row.get::<_, Option<String>>(5)?,
        cache_key: row.get::<_, Option<String>>(6)?,
        created_at: row.get::<_, i64>(7)?,
        updated_at: row.get::<_, i64>(8)?,
    })
}

fn completed_step_ids(steps: &[StoredFlowStep]) -> BTreeSet<String> {
    steps
        .iter()
        .filter(|step| step.status == StepStatus::Completed.as_str())
        .map(|step| step.id.clone())
        .collect()
}

fn parse_step_ref(step_ref: &str) -> Option<(String, String)> {
    let trimmed = step_ref.trim();
    if let Some(rest) = trimmed.strip_prefix("step:") {
        return split_step_ref_pair(rest, '/');
    }
    if let Some((flow_id, step_id)) = trimmed.split_once('/') {
        return non_empty_step_ref_pair(flow_id, step_id);
    }
    trimmed
        .split_once('.')
        .and_then(|(flow_id, step_id)| non_empty_step_ref_pair(flow_id, step_id))
}

fn split_step_ref_pair(input: &str, separator: char) -> Option<(String, String)> {
    input
        .split_once(separator)
        .and_then(|(flow_id, step_id)| non_empty_step_ref_pair(flow_id, step_id))
}

fn non_empty_step_ref_pair(flow_id: &str, step_id: &str) -> Option<(String, String)> {
    let flow_id = flow_id.trim();
    let step_id = step_id.trim();
    if flow_id.is_empty() || step_id.is_empty() {
        None
    } else {
        Some((flow_id.to_string(), step_id.to_string()))
    }
}

fn local_step_id(db_step_id: &str) -> String {
    db_step_id
        .rsplit_once('/')
        .map_or_else(|| db_step_id.to_string(), |(_, local)| local.to_string())
}

/// A step that has been prepared (inputs resolved, workdir materialized, run/
/// attempt rows inserted) and now only needs its tool subprocess run, then its
/// result recorded. The subprocess (`run_local_command`) touches no database, so
/// a wave of these can execute concurrently; preparation and recording stay on
/// the main thread, keeping the single SQLite connection race-free.
struct PendingStep {
    command: Option<Command>,
    timeout_seconds: Option<u64>,
    tool_ref: String,
    tool_outputs: BTreeMap<String, crate::storage::ToolPortSpec>,
    run_id: String,
    attempt_id: String,
    step_id: String,
    workdir: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    resolved_outputs: OutputPaths,
    cache_key: String,
    input_hashes_json: String,
    params_hash: String,
    runtime_hash: String,
}

enum PreparedStep {
    /// Already finished during preparation (cache hit or a pre-run validation
    /// failure) — the attempt is fully recorded.
    Finished(AttemptSummary),
    /// Needs its subprocess run and the result recorded.
    Pending(Box<PendingStep>),
}

struct OutputPaths {
    root: PathBuf,
    paths: BTreeMap<String, PathBuf>,
}

impl OutputPaths {
    fn root(&self) -> &Path {
        &self.root
    }

    fn as_map(&self) -> &BTreeMap<String, PathBuf> {
        &self.paths
    }
}

struct ResolvedInput {
    path: PathBuf,
    cache_identity: String,
}

struct ResolvedStepOutput {
    path: PathBuf,
    cache_identity: String,
}

struct FinishAttempt {
    run_id: String,
    attempt_id: String,
    step_id: String,
    workdir: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    status: RunAttemptStatus,
    exit_code: Option<i32>,
    error_message: Option<String>,
}

struct CacheEntryWrite {
    cache_key: String,
    tool_ref: String,
    input_hashes_json: String,
    params_hash: String,
    runtime_hash: String,
    output_artifacts_json: String,
}

struct DependencyPackageDiff {
    missing: Vec<String>,
    extra: Vec<String>,
}

impl EnvironmentCheckItem {
    fn ok(name: impl Into<String>, message: impl Into<String>, details: Option<String>) -> Self {
        Self {
            name: name.into(),
            status: "ok".to_string(),
            message: message.into(),
            details,
        }
    }

    fn failed(
        name: impl Into<String>,
        message: impl Into<String>,
        details: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status: "failed".to_string(),
            message: message.into(),
            details,
        }
    }

    fn skipped(
        name: impl Into<String>,
        message: impl Into<String>,
        details: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status: "skipped".to_string(),
            message: message.into(),
            details,
        }
    }
}

struct LocalCommandOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
    timeout_seconds: Option<u64>,
}

fn provisioner_command_error(action: &str, output: &LocalCommandOutput) -> StorageError {
    StorageError::InvalidInput(format!(
        "isolated environment {action} command exited with code {:?}: stdout={} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedRuntimeCommand {
    executable: String,
    args: Vec<String>,
}

impl PreparedRuntimeCommand {
    fn argv(&self) -> Vec<String> {
        let mut argv = Vec::with_capacity(self.args.len() + 1);
        argv.push(self.executable.clone());
        argv.extend(self.args.clone());
        argv
    }
}

fn output_paths(workdir: &Path, outputs: &BTreeMap<String, String>) -> OutputPaths {
    let root = workdir.join("outputs");
    let paths = outputs
        .iter()
        .map(|(name, alias)| {
            let output_name = if alias.trim().is_empty() {
                name.as_str()
            } else {
                alias.as_str()
            };
            (name.clone(), root.join(sanitize_path_part(output_name)))
        })
        .collect();
    OutputPaths { root, paths }
}

fn input_paths(
    inputs: &BTreeMap<String, ResolvedInput>,
    workdir: &Path,
    force_copy: bool,
) -> Result<BTreeMap<String, PathBuf>, StorageError> {
    let inputs_root = workdir.join("inputs");
    fs::create_dir_all(&inputs_root)?;
    inputs
        .iter()
        .map(|(name, input)| {
            stage_input_path(&input.path, &inputs_root, name, force_copy)
                .map(|staged_path| (name.clone(), staged_path))
        })
        .collect()
}

fn stage_input_path(
    source_path: &Path,
    inputs_root: &Path,
    port_name: &str,
    force_copy: bool,
) -> Result<PathBuf, StorageError> {
    // Container backends mount only the per-step workdir, so a symlink into the
    // artifact store (outside the mount) would dangle inside the container.
    // Stage real file copies for them; other backends keep the lighter symlink
    // (with copy fallback) logical boundary.
    if force_copy {
        stage_input_path_with_linker(source_path, inputs_root, port_name, copy_input_file)
    } else {
        stage_input_path_with_linker(source_path, inputs_root, port_name, create_input_symlink)
    }
}

fn copy_input_file(source_path: &Path, staged_path: &Path) -> io::Result<()> {
    fs::copy(source_path, staged_path).map(|_| ())
}

fn stage_input_path_with_linker<F>(
    source_path: &Path,
    inputs_root: &Path,
    port_name: &str,
    link: F,
) -> Result<PathBuf, StorageError>
where
    F: Fn(&Path, &Path) -> io::Result<()>,
{
    let port_dir = inputs_root.join(sanitize_path_part(port_name));
    fs::create_dir_all(&port_dir)?;
    let filename = source_path
        .file_name()
        .and_then(OsStr::to_str)
        .map(sanitize_path_part)
        .unwrap_or_else(|| "input".to_string());
    let staged_path = port_dir.join(filename);

    // Local/conda staging is a logical workdir boundary: tools receive only
    // workdir paths, but symlinks can still be followed to the artifact store.
    // Hard filesystem isolation belongs to the container backend.
    match link(source_path, &staged_path) {
        Ok(()) => Ok(staged_path),
        Err(link_error) => match fs::copy(source_path, &staged_path) {
            Ok(_) => Ok(staged_path),
            Err(copy_error) => Err(StorageError::InvalidInput(format!(
                "failed to stage input port {port_name} from {} to {}: symlink failed ({link_error}); copy failed ({copy_error})",
                source_path.display(),
                staged_path.display()
            ))),
        },
    }
}

#[cfg(unix)]
fn create_input_symlink(source_path: &Path, staged_path: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source_path, staged_path)
}

#[cfg(windows)]
fn create_input_symlink(source_path: &Path, staged_path: &Path) -> io::Result<()> {
    if source_path.is_dir() {
        std::os::windows::fs::symlink_dir(source_path, staged_path)
    } else {
        std::os::windows::fs::symlink_file(source_path, staged_path)
    }
}

fn input_hashes_json(inputs: &BTreeMap<String, ResolvedInput>) -> String {
    let map = inputs
        .iter()
        .map(|(name, input)| (name.clone(), input.cache_identity.clone()))
        .collect::<BTreeMap<_, _>>();
    string_map_json(&map)
}

fn compute_cache_key(
    tool_ref: &str,
    tool_version: &str,
    input_hashes_json: &str,
    params_hash: &str,
    runtime_hash: &str,
) -> String {
    stable_hash(&format!(
        "tool={tool_ref}@{tool_version};inputs={input_hashes_json};params={params_hash};runtime={runtime_hash}"
    ))
}

fn runtime_config_json(runtime: &ToolRuntimeSpec) -> Result<String, StorageError> {
    runtime_config_json_with_isolated_lock(runtime, None)
}

fn runtime_config_json_with_isolated_lock(
    runtime: &ToolRuntimeSpec,
    isolated_env_lock: Option<String>,
) -> Result<String, StorageError> {
    let env_file_hash = runtime
        .env_file
        .as_deref()
        .map(|path| file_hash_fnv64(Path::new(path)))
        .transpose()?;
    serde_json::to_string(&RuntimeConfigJson {
        backend: runtime.backend.clone(),
        command: runtime.command.clone(),
        timeout_seconds: runtime.timeout_seconds,
        env_name: runtime.env_name.clone(),
        env_prefix: runtime.env_prefix.clone(),
        env_file: runtime.env_file.clone(),
        env_file_hash,
        runner: runtime.runner.clone(),
        container_image: if runtime.backend == "container" {
            runtime.image.clone()
        } else {
            None
        },
        isolated_env_lock,
    })
    .map_err(|err| StorageError::InvalidInput(format!("runtime config JSON failed: {err}")))
}

#[derive(Debug, Serialize, Deserialize)]
struct RuntimeConfigJson {
    backend: String,
    command: Vec<String>,
    timeout_seconds: Option<u64>,
    env_name: Option<String>,
    env_prefix: Option<String>,
    env_file: Option<String>,
    env_file_hash: Option<String>,
    runner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    container_image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    isolated_env_lock: Option<String>,
}

fn stable_hash(input: &str) -> String {
    stable_hash_bytes(input.as_bytes())
}

fn file_hash_fnv64(path: &Path) -> Result<String, StorageError> {
    Ok(stable_hash_bytes(&fs::read(path)?))
}

fn isolated_env_lock_hash(env_file: &Path) -> Result<String, StorageError> {
    isolated_env_lock_hash_for_platform(env_file, &platform_tag())
}

fn isolated_env_lock_hash_for_platform(
    env_file: &Path,
    platform_tag: &str,
) -> Result<String, StorageError> {
    let mut bytes = fs::read(env_file)?;
    bytes.extend_from_slice(platform_tag.as_bytes());
    Ok(stable_hash_bytes(&bytes))
}

fn platform_tag() -> String {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "osx-arm64".to_string(),
        ("macos", "x86_64") => "osx-64".to_string(),
        ("linux", "x86_64") => "linux-64".to_string(),
        ("linux", "aarch64") => "linux-aarch64".to_string(),
        ("windows", "x86_64") => "win-64".to_string(),
        (os, arch) => format!("{os}-{arch}"),
    }
}

fn isolated_env_prefix(project_root: &Path, tool_ref: &str, lock_hash: &str) -> PathBuf {
    project_dir(project_root).join("envs").join(format!(
        "{}@{}",
        path_safe_tool_id(tool_ref),
        lock_hash
    ))
}

fn path_safe_tool_id(tool_ref: &str) -> String {
    tool_ref
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn stable_hash_bytes(bytes: &[u8]) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("fnv64:{hash:016x}")
}

fn materialize_workdir(
    workdir: &Path,
    command: &[String],
    runtime_config_json: &str,
    inputs: &BTreeMap<String, PathBuf>,
    params: &BTreeMap<String, String>,
    outputs: &BTreeMap<String, PathBuf>,
) -> Result<(), StorageError> {
    fs::write(
        workdir.join("command.sh"),
        format!("{}\n", shell_display(command)),
    )?;
    fs::write(workdir.join("inputs.json"), path_map_json(inputs))?;
    fs::write(workdir.join("params.json"), string_map_json(params))?;
    fs::write(workdir.join("outputs.json"), path_map_json(outputs))?;
    fs::write(workdir.join("runtime.json"), runtime_config_json)?;
    Ok(())
}

fn isolated_env_state_for_tool(
    project_root: &Path,
    tool_ref: &str,
    runtime: &ToolRuntimeSpec,
) -> Result<Option<IsolatedEnvState>, StorageError> {
    if runtime.backend != ISOLATED_ENV_BACKEND {
        return Ok(None);
    }
    let env_file = runtime.env_file.as_deref().ok_or_else(|| {
        StorageError::InvalidInput("isolated runtime must declare runtime.env_file".to_string())
    })?;
    let lock_hash = isolated_env_lock_hash(Path::new(env_file))?;
    Ok(Some(IsolatedEnvState {
        prefix: isolated_env_prefix(project_root, tool_ref, &lock_hash),
        lock_hash,
    }))
}

fn ensure_isolated_tool_environment(
    project_root: &Path,
    tool_ref: &str,
    runtime: &ToolRuntimeSpec,
    provisioner: &dyn IsolatedEnvProvisioner,
) -> Result<Option<IsolatedEnvState>, StorageError> {
    if runtime.backend != ISOLATED_ENV_BACKEND {
        return Ok(None);
    }
    ensure_isolated_tool_environment_for_platform(
        project_root,
        tool_ref,
        runtime,
        &platform_tag(),
        provisioner,
    )
    .map(Some)
}

fn ensure_isolated_tool_environment_for_platform(
    project_root: &Path,
    tool_ref: &str,
    runtime: &ToolRuntimeSpec,
    platform_tag: &str,
    provisioner: &dyn IsolatedEnvProvisioner,
) -> Result<IsolatedEnvState, StorageError> {
    if runtime.backend != ISOLATED_ENV_BACKEND {
        return Err(StorageError::InvalidInput(format!(
            "isolated environment requested for unsupported backend {}",
            runtime.backend
        )));
    }
    if runtime.env_name.is_some() || runtime.env_prefix.is_some() {
        return Err(StorageError::InvalidInput(
            "isolated runtime must not declare env_name or env_prefix".to_string(),
        ));
    }
    let env_file = runtime.env_file.as_deref().ok_or_else(|| {
        StorageError::InvalidInput("isolated runtime must declare runtime.env_file".to_string())
    })?;
    let runner = runtime.runner.as_deref().ok_or_else(|| {
        StorageError::InvalidInput(
            "environment runtime must declare absolute runner path".to_string(),
        )
    })?;
    let env_file = Path::new(env_file);
    let lock_hash = isolated_env_lock_hash_for_platform(env_file, platform_tag)?;
    let prefix = isolated_env_prefix(project_root, tool_ref, &lock_hash);
    let lock_path = prefix.join(ISOLATED_ENV_LOCK_FILE);
    if prefix.exists() && lock_path.is_file() {
        return Ok(IsolatedEnvState { prefix, lock_hash });
    }

    provisioner.ensure(env_file, &prefix, runner)?;
    if !lock_path.is_file() {
        return Err(StorageError::InvalidInput(format!(
            "isolated environment provisioner did not write {}",
            lock_path.display()
        )));
    }
    Ok(IsolatedEnvState { prefix, lock_hash })
}

fn runtime_config_json_for_tool(
    project_root: &Path,
    tool_ref: &str,
    runtime: &ToolRuntimeSpec,
) -> Result<String, StorageError> {
    if runtime.backend != ISOLATED_ENV_BACKEND {
        return runtime_config_json(runtime);
    }
    let isolated = isolated_env_state_for_tool(project_root, tool_ref, runtime)?;
    runtime_config_json_with_isolated_lock(runtime, isolated.map(|state| state.lock_hash))
}

fn prepare_runtime_command(
    runtime: &ToolRuntimeSpec,
    ctx: &backend::ExecContext<'_>,
) -> Result<PreparedRuntimeCommand, StorageError> {
    match backend::backend_for(&runtime.backend) {
        Some(backend) => backend.prepare_command(runtime, ctx),
        None => Err(StorageError::InvalidInput(format!(
            "unsupported runtime.backend {}",
            runtime.backend
        ))),
    }
}

fn prepare_runtime_command_for_tool(
    runtime: &ToolRuntimeSpec,
    isolated_env: Option<&IsolatedEnvState>,
    ctx: &backend::ExecContext<'_>,
) -> Result<PreparedRuntimeCommand, StorageError> {
    if runtime.backend != ISOLATED_ENV_BACKEND {
        return prepare_runtime_command(runtime, ctx);
    }
    let isolated_env = isolated_env.ok_or_else(|| {
        StorageError::InvalidInput(
            "isolated runtime command requires a managed environment prefix".to_string(),
        )
    })?;
    let mut managed_runtime = runtime.clone();
    managed_runtime.env_name = None;
    managed_runtime.env_prefix = Some(isolated_env.prefix.display().to_string());
    prepare_runtime_command(&managed_runtime, ctx)
}

fn check_runner(runtime: &ToolRuntimeSpec, items: &mut Vec<EnvironmentCheckItem>) {
    let Some(runner) = runtime.runner.as_deref() else {
        items.push(EnvironmentCheckItem::failed(
            "runner",
            "environment runtime does not declare runner",
            None,
        ));
        return;
    };
    let path = Path::new(runner);
    match fs::metadata(path) {
        Ok(metadata) if !metadata.is_file() => items.push(EnvironmentCheckItem::failed(
            "runner",
            format!("runner is not a file: {runner}"),
            None,
        )),
        Ok(metadata) if !is_executable(&metadata) => items.push(EnvironmentCheckItem::failed(
            "runner",
            format!("runner is not executable: {runner}"),
            None,
        )),
        Ok(_) => items.push(EnvironmentCheckItem::ok(
            "runner",
            format!("runner is available: {runner}"),
            None,
        )),
        Err(error) => items.push(EnvironmentCheckItem::failed(
            "runner",
            format!("runner is not accessible: {runner}"),
            Some(error.to_string()),
        )),
    }
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(metadata: &fs::Metadata) -> bool {
    !metadata.permissions().readonly()
}

fn check_environment_selector(runtime: &ToolRuntimeSpec, items: &mut Vec<EnvironmentCheckItem>) {
    match (runtime.env_name.as_deref(), runtime.env_prefix.as_deref()) {
        (Some(env_name), None) => items.push(EnvironmentCheckItem::ok(
            "environment",
            format!("using environment name {env_name}"),
            None,
        )),
        (None, Some(env_prefix)) => {
            let path = Path::new(env_prefix);
            if path.exists() {
                items.push(EnvironmentCheckItem::ok(
                    "environment",
                    format!("using environment prefix {env_prefix}"),
                    None,
                ));
            } else {
                items.push(EnvironmentCheckItem::failed(
                    "environment",
                    format!("environment prefix does not exist: {env_prefix}"),
                    None,
                ));
            }
        }
        (Some(_), Some(_)) => items.push(EnvironmentCheckItem::failed(
            "environment",
            "runtime declares both env_name and env_prefix",
            None,
        )),
        (None, None) => items.push(EnvironmentCheckItem::failed(
            "environment",
            "runtime declares neither env_name nor env_prefix",
            None,
        )),
    }
}

fn check_prepare_environment_selector(
    runtime: &ToolRuntimeSpec,
    items: &mut Vec<EnvironmentCheckItem>,
) {
    match (runtime.env_name.as_deref(), runtime.env_prefix.as_deref()) {
        (Some(env_name), None) => items.push(EnvironmentCheckItem::ok(
            "environment",
            format!("preparing environment name {env_name}"),
            None,
        )),
        (None, Some(env_prefix)) => items.push(EnvironmentCheckItem::ok(
            "environment",
            format!("preparing environment prefix {env_prefix}"),
            None,
        )),
        (Some(_), Some(_)) => items.push(EnvironmentCheckItem::failed(
            "environment",
            "runtime declares both env_name and env_prefix",
            None,
        )),
        (None, None) => items.push(EnvironmentCheckItem::failed(
            "environment",
            "runtime declares neither env_name nor env_prefix",
            None,
        )),
    }
}

fn check_isolated_runtime_selector(
    runtime: &ToolRuntimeSpec,
    items: &mut Vec<EnvironmentCheckItem>,
) {
    if runtime.env_name.is_some() || runtime.env_prefix.is_some() {
        items.push(EnvironmentCheckItem::failed(
            "environment",
            "isolated runtime must not declare env_name or env_prefix",
            None,
        ));
    } else {
        items.push(EnvironmentCheckItem::ok(
            "environment",
            "using AgentFlow-managed isolated environment prefix",
            None,
        ));
    }
}

fn check_isolated_environment(
    project_root: &Path,
    tool_ref: &str,
    runtime: &ToolRuntimeSpec,
    items: &mut Vec<EnvironmentCheckItem>,
) {
    check_isolated_runtime_selector(runtime, items);
    check_required_env_file(runtime, items);
    if items
        .iter()
        .any(|item| matches!(item.name.as_str(), "environment" | "env_file") && item.status != "ok")
    {
        return;
    }
    match isolated_env_state_for_tool(project_root, tool_ref, runtime) {
        Ok(Some(state)) => {
            let lock_path = state.prefix.join(ISOLATED_ENV_LOCK_FILE);
            let status = if lock_path.is_file() {
                EnvironmentCheckItem::ok(
                    "managed_env",
                    "isolated environment lock exists",
                    Some(format!(
                        "prefix={}; lock_hash={}",
                        state.prefix.display(),
                        state.lock_hash
                    )),
                )
            } else {
                EnvironmentCheckItem::skipped(
                    "managed_env",
                    "isolated environment has not been prepared",
                    Some(format!(
                        "prefix={}; lock_hash={}",
                        state.prefix.display(),
                        state.lock_hash
                    )),
                )
            };
            items.push(status);
        }
        Ok(None) => items.push(EnvironmentCheckItem::failed(
            "managed_env",
            "isolated environment state unavailable",
            None,
        )),
        Err(error) => items.push(EnvironmentCheckItem::failed(
            "managed_env",
            "isolated environment metadata failed",
            Some(error.to_string()),
        )),
    }
}

fn check_env_file(runtime: &ToolRuntimeSpec, items: &mut Vec<EnvironmentCheckItem>) {
    let Some(env_file) = runtime.env_file.as_deref() else {
        items.push(EnvironmentCheckItem::ok(
            "env_file",
            "no env_file declared; using existing environment only",
            None,
        ));
        return;
    };
    match file_hash_fnv64(Path::new(env_file)) {
        Ok(hash) => items.push(EnvironmentCheckItem::ok(
            "env_file",
            format!("env_file is readable: {env_file}"),
            Some(hash),
        )),
        Err(error) => items.push(EnvironmentCheckItem::failed(
            "env_file",
            format!("env_file is not readable: {env_file}"),
            Some(error.to_string()),
        )),
    }
}

fn check_required_env_file(runtime: &ToolRuntimeSpec, items: &mut Vec<EnvironmentCheckItem>) {
    let Some(env_file) = runtime.env_file.as_deref() else {
        items.push(EnvironmentCheckItem::failed(
            "env_file",
            "env prepare requires runtime.env_file",
            None,
        ));
        return;
    };
    match file_hash_fnv64(Path::new(env_file)) {
        Ok(hash) => items.push(EnvironmentCheckItem::ok(
            "env_file",
            format!("env_file is readable: {env_file}"),
            Some(hash),
        )),
        Err(error) => items.push(EnvironmentCheckItem::failed(
            "env_file",
            format!("env_file is not readable: {env_file}"),
            Some(error.to_string()),
        )),
    }
}

fn isolated_environment_create_argv(
    runtime: &ToolRuntimeSpec,
    prefix: &Path,
) -> Result<Vec<String>, StorageError> {
    let runner = runtime.runner.as_ref().ok_or_else(|| {
        StorageError::InvalidInput(
            "environment runtime must declare absolute runner path".to_string(),
        )
    })?;
    let env_file = runtime.env_file.as_ref().ok_or_else(|| {
        StorageError::InvalidInput("isolated runtime must declare runtime.env_file".to_string())
    })?;
    Ok(vec![
        runner.clone(),
        "create".to_string(),
        "-y".to_string(),
        "-p".to_string(),
        prefix.display().to_string(),
        "-f".to_string(),
        env_file.clone(),
    ])
}

fn check_environment_probe(runtime: &ToolRuntimeSpec, items: &mut Vec<EnvironmentCheckItem>) {
    if items
        .iter()
        .any(|item| matches!(item.name.as_str(), "runner" | "environment") && item.status != "ok")
    {
        items.push(EnvironmentCheckItem::skipped(
            "probe",
            "environment probe skipped because runner or environment metadata failed",
            None,
        ));
        return;
    }
    let probe = true_command();
    let Some(command) = prepare_environment_probe(runtime, &probe) else {
        items.push(EnvironmentCheckItem::failed(
            "probe",
            "cannot prepare environment probe",
            None,
        ));
        return;
    };
    let mut process = Command::new(&command.executable);
    process
        .args(&command.args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin");
    match run_local_command(process, Some(15)) {
        Ok(output) if output.timed_out => items.push(EnvironmentCheckItem::failed(
            "probe",
            "environment probe timed out after 15 seconds",
            Some(String::from_utf8_lossy(&output.stderr).to_string()),
        )),
        Ok(output) if output.status.success() => items.push(EnvironmentCheckItem::ok(
            "probe",
            "environment probe command succeeded",
            None,
        )),
        Ok(output) => items.push(EnvironmentCheckItem::failed(
            "probe",
            format!(
                "environment probe exited with code {:?}",
                output.status.code()
            ),
            Some(String::from_utf8_lossy(&output.stderr).to_string()),
        )),
        Err(error) => items.push(EnvironmentCheckItem::failed(
            "probe",
            "environment probe could not start",
            Some(error.to_string()),
        )),
    }
}

fn prepare_environment_update_command(
    runtime: &ToolRuntimeSpec,
) -> Result<PreparedRuntimeCommand, StorageError> {
    let runner = runtime.runner.as_ref().ok_or_else(|| {
        StorageError::InvalidInput(
            "environment runtime must declare absolute runner path".to_string(),
        )
    })?;
    let env_file = runtime.env_file.as_ref().ok_or_else(|| {
        StorageError::InvalidInput("env prepare requires runtime.env_file".to_string())
    })?;
    let mut args = vec!["env".to_string(), "update".to_string()];
    match (runtime.env_name.as_deref(), runtime.env_prefix.as_deref()) {
        (Some(env_name), None) => {
            args.push("--name".to_string());
            args.push(env_name.to_string());
        }
        (None, Some(env_prefix)) => {
            args.push("--prefix".to_string());
            args.push(env_prefix.to_string());
        }
        (Some(_), Some(_)) => {
            return Err(StorageError::InvalidInput(
                "environment runtime must declare only one of env_name or env_prefix".to_string(),
            ));
        }
        (None, None) => {
            return Err(StorageError::InvalidInput(
                "environment runtime must declare env_name or env_prefix".to_string(),
            ));
        }
    }
    args.push("--file".to_string());
    args.push(env_file.clone());
    Ok(PreparedRuntimeCommand {
        executable: runner.clone(),
        args,
    })
}

fn prepare_environment_export_command(
    runtime: &ToolRuntimeSpec,
) -> Result<PreparedRuntimeCommand, StorageError> {
    let runner = runtime.runner.as_ref().ok_or_else(|| {
        StorageError::InvalidInput(
            "environment runtime must declare absolute runner path".to_string(),
        )
    })?;
    let mut args = vec!["env".to_string(), "export".to_string()];
    match (runtime.env_name.as_deref(), runtime.env_prefix.as_deref()) {
        (Some(env_name), None) => {
            args.push("--name".to_string());
            args.push(env_name.to_string());
        }
        (None, Some(env_prefix)) => {
            args.push("--prefix".to_string());
            args.push(env_prefix.to_string());
        }
        (Some(_), Some(_)) => {
            return Err(StorageError::InvalidInput(
                "environment runtime must declare only one of env_name or env_prefix".to_string(),
            ));
        }
        (None, None) => {
            return Err(StorageError::InvalidInput(
                "environment runtime must declare env_name or env_prefix".to_string(),
            ));
        }
    }
    Ok(PreparedRuntimeCommand {
        executable: runner.clone(),
        args,
    })
}

fn declared_environment_packages(runtime: &ToolRuntimeSpec) -> Result<Vec<String>, StorageError> {
    let Some(env_file) = runtime.env_file.as_deref() else {
        return Ok(Vec::new());
    };
    Ok(extract_conda_dependency_packages(
        &read_text_file_with_byte_cap(
            "environment YAML",
            Path::new(env_file),
            MAX_RUNTIME_ENVIRONMENT_YAML_BYTES,
        )?,
    ))
}

fn extract_conda_dependency_packages(text: &str) -> Vec<String> {
    let mut packages = BTreeSet::new();
    let mut in_dependencies = false;
    let mut dependency_list_indent = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len().saturating_sub(line.trim_start().len());
        if trimmed == "dependencies:" {
            in_dependencies = true;
            dependency_list_indent = None;
            continue;
        }
        if !in_dependencies {
            continue;
        }
        if !trimmed.starts_with("- ") {
            if indent == 0 {
                in_dependencies = false;
            }
            continue;
        }
        if let Some(list_indent) = dependency_list_indent {
            if indent != list_indent {
                continue;
            }
        } else {
            dependency_list_indent = Some(indent);
        }
        let Some(package) = normalize_dependency_package(trimmed.trim_start_matches("- ")) else {
            continue;
        };
        packages.insert(package);
    }
    packages.into_iter().collect()
}

fn normalize_dependency_package(value: &str) -> Option<String> {
    let value = value
        .split_once(" #")
        .map(|(left, _)| left)
        .unwrap_or(value)
        .trim()
        .trim_matches('"')
        .trim_matches('\'');
    if value.is_empty() || value.ends_with(':') {
        return None;
    }
    let value = value
        .rsplit_once("::")
        .map(|(_, name)| name)
        .unwrap_or(value);
    let name = value
        .split(['=', '<', '>', '!', ' ', '\t'])
        .next()
        .unwrap_or_default()
        .trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_ascii_lowercase())
    }
}

fn dependency_package_diff(
    declared_packages: &[String],
    exported_packages: &[String],
) -> DependencyPackageDiff {
    let declared = declared_packages.iter().cloned().collect::<BTreeSet<_>>();
    let exported = exported_packages.iter().cloned().collect::<BTreeSet<_>>();
    DependencyPackageDiff {
        missing: declared.difference(&exported).cloned().collect(),
        extra: exported.difference(&declared).cloned().collect(),
    }
}

fn environment_package_diff_item(
    declared_count: usize,
    exported_count: usize,
    missing_packages: &[String],
    extra_packages: &[String],
) -> EnvironmentCheckItem {
    if declared_count == 0 {
        return EnvironmentCheckItem::skipped(
            "package_diff",
            "no declared env_file dependencies available for package diff",
            Some(format!("exported_packages={exported_count}")),
        );
    }
    if missing_packages.is_empty() && extra_packages.is_empty() {
        return EnvironmentCheckItem::ok(
            "package_diff",
            "exported package set matches declared package set",
            Some(format!(
                "declared_packages={declared_count}; exported_packages={exported_count}"
            )),
        );
    }
    if !missing_packages.is_empty() {
        return EnvironmentCheckItem::failed(
            "package_diff",
            "exported package set is missing declared packages",
            Some(format!(
                "missing={}; extra={}",
                dependency_list_details(missing_packages),
                dependency_list_details(extra_packages)
            )),
        );
    }
    EnvironmentCheckItem::ok(
        "package_diff",
        "exported package set includes packages beyond the declared set",
        Some(format!(
            "missing={}; extra={}",
            dependency_list_details(missing_packages),
            dependency_list_details(extra_packages)
        )),
    )
}

fn dependency_list_details(packages: &[String]) -> String {
    if packages.is_empty() {
        "none".to_string()
    } else {
        packages.join(",")
    }
}

fn prepare_environment_probe(
    runtime: &ToolRuntimeSpec,
    probe_command: &str,
) -> Option<PreparedRuntimeCommand> {
    let runner = runtime.runner.as_ref()?;
    let mut args = vec!["run".to_string()];
    if runtime.backend == "conda" {
        args.push("--no-capture-output".to_string());
    }
    match (runtime.env_name.as_deref(), runtime.env_prefix.as_deref()) {
        (Some(env_name), None) => {
            args.push("--name".to_string());
            args.push(env_name.to_string());
        }
        (None, Some(env_prefix)) => {
            args.push("--prefix".to_string());
            args.push(env_prefix.to_string());
        }
        _ => return None,
    }
    args.push(probe_command.to_string());
    Some(PreparedRuntimeCommand {
        executable: runner.clone(),
        args,
    })
}

fn install_runtime_python_egress_guard(workdir: &Path) -> Result<PathBuf, StorageError> {
    let guard_dir = runtime_python_egress_guard_dir(workdir);
    fs::create_dir_all(&guard_dir)?;
    fs::write(
        guard_dir.join("sitecustomize.py"),
        PYTHON_EGRESS_GUARD_SITECUSTOMIZE.as_bytes(),
    )?;
    Ok(guard_dir)
}

fn runtime_python_egress_guard_dir(workdir: &Path) -> PathBuf {
    workdir.join("python-egress-guard")
}

fn prepend_python_egress_guard_to_runtime_command(
    command: &mut PreparedRuntimeCommand,
    guard_dir: &Path,
) -> Result<Option<OsString>, StorageError> {
    if is_env_executable(&command.executable) {
        for arg in &mut command.args {
            let Some(existing) = arg.strip_prefix("PYTHONPATH=") else {
                continue;
            };
            let pythonpath = pythonpath_with_runtime_guard(guard_dir, Some(OsStr::new(existing)))?;
            *arg = format!("PYTHONPATH={}", pythonpath.to_string_lossy());
            return Ok(None);
        }
    }
    pythonpath_with_runtime_guard(guard_dir, None).map(Some)
}

fn is_env_executable(executable: &str) -> bool {
    Path::new(executable).file_name().and_then(OsStr::to_str) == Some("env")
}

fn pythonpath_with_runtime_guard(
    guard_dir: &Path,
    existing: Option<&OsStr>,
) -> Result<OsString, StorageError> {
    let mut paths = vec![guard_dir.to_path_buf()];
    if let Some(existing) = existing {
        paths.extend(std::env::split_paths(existing));
    }
    std::env::join_paths(paths).map_err(|error| {
        StorageError::InvalidInput(format!("failed to build Python egress guard path: {error}"))
    })
}

fn true_command() -> String {
    ["/usr/bin/true", "/bin/true"]
        .iter()
        .find(|path| Path::new(path).exists())
        .map(|path| (*path).to_string())
        .unwrap_or_else(|| "true".to_string())
}

fn run_local_command(
    mut command: Command,
    timeout_seconds: Option<u64>,
) -> std::io::Result<LocalCommandOutput> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let Some(timeout_seconds) = timeout_seconds else {
        let child = command.spawn()?;
        return wait_for_local_command_output(child, None);
    };

    configure_timeout_process_group(&mut command);
    let child = command.spawn()?;
    wait_for_local_command_output(child, Some(timeout_seconds))
}

fn wait_for_local_command_output(
    mut child: std::process::Child,
    timeout_seconds: Option<u64>,
) -> io::Result<LocalCommandOutput> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("child stdout was not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("child stderr was not piped"))?;
    let stdout_handle = thread::spawn(move || {
        capture_stream_with_byte_cap(stdout, MAX_RUNTIME_CAPTURE_BYTES, "stdout")
    });
    let stderr_handle = thread::spawn(move || {
        capture_stream_with_byte_cap(stderr, MAX_RUNTIME_CAPTURE_BYTES, "stderr")
    });

    let (status, timed_out) = if let Some(timeout_seconds) = timeout_seconds {
        let timeout = Duration::from_secs(timeout_seconds);
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                break (status, false);
            }
            if started.elapsed() >= timeout {
                kill_timeout_process_group(&mut child);
                break (child.wait()?, true);
            }
            thread::sleep(Duration::from_millis(25));
        }
    } else {
        (child.wait()?, false)
    };

    let stdout = join_capture_thread(stdout_handle, "stdout")?;
    let stderr = join_capture_thread(stderr_handle, "stderr")?;
    Ok(LocalCommandOutput {
        status,
        stdout,
        stderr,
        timed_out,
        timeout_seconds,
    })
}

fn join_capture_thread(
    handle: thread::JoinHandle<io::Result<Vec<u8>>>,
    stream_name: &str,
) -> io::Result<Vec<u8>> {
    handle
        .join()
        .map_err(|_| io::Error::other(format!("{stream_name} capture thread panicked")))?
}

fn capture_stream_with_byte_cap<R: Read>(
    reader: R,
    max_bytes: usize,
    stream_name: &str,
) -> io::Result<Vec<u8>> {
    let mut reader = BufReader::new(reader);
    let mut buffer = [0_u8; 8192];
    let mut captured = Vec::new();
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(captured.len());
        if remaining > 0 {
            let keep = remaining.min(count);
            captured.extend_from_slice(&buffer[..keep]);
        }
        if count > remaining {
            truncated = true;
        }
    }
    if truncated {
        captured.extend_from_slice(
            format!("\n[agentflow] {stream_name} truncated after {max_bytes} bytes\n").as_bytes(),
        );
    }
    Ok(captured)
}

fn configure_timeout_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
}

fn kill_timeout_process_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .arg("-TERM")
            .arg(format!("-{}", child.id()))
            .status();
    }
    let _ = child.kill();
}

fn validate_outputs(outputs: &OutputPaths) -> Result<(), StorageError> {
    let output_root = fs::canonicalize(outputs.root())?;
    for (name, path) in outputs.as_map() {
        let metadata = fs::metadata(path).map_err(|_| {
            StorageError::InvalidInput(format!(
                "declared output {name} does not exist: {}",
                path.display()
            ))
        })?;
        let canonical_path = fs::canonicalize(path)?;
        if !canonical_path.starts_with(&output_root) {
            return Err(StorageError::InvalidInput(format!(
                "declared output {name} escapes workdir outputs: {}",
                canonical_path.display()
            )));
        }
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(StorageError::InvalidInput(format!(
                "declared output {name} must be a non-empty file: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn write_runtime_error(stderr_path: &Path, error: &StorageError) -> String {
    let message = error.to_string();
    let existing =
        read_text_file_with_byte_cap("stderr log", stderr_path, MAX_RUNTIME_LOG_READ_BYTES)
            .unwrap_or_default();
    let updated = if existing.trim().is_empty() {
        format!("{message}\n")
    } else if existing.ends_with('\n') {
        format!("{existing}{message}\n")
    } else {
        format!("{existing}\n{message}\n")
    };
    let _ = fs::write(stderr_path, updated);
    message
}

fn read_text_file_with_byte_cap(
    kind: &str,
    path: &Path,
    max_bytes: u64,
) -> Result<String, StorageError> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > max_bytes {
        return Err(StorageError::InvalidInput(format!(
            "{kind} exceeds {max_bytes} byte cap at {}",
            path.display()
        )));
    }
    let mut text = String::new();
    BufReader::new(fs::File::open(path)?)
        .read_to_string(&mut text)
        .map_err(|error| {
            StorageError::InvalidInput(format!(
                "{kind} requires UTF-8 text at {}: {error}",
                path.display()
            ))
        })?;
    Ok(text)
}

fn read_trimmed_nonempty_lines_with_byte_cap(
    kind: &str,
    path: &Path,
    max_bytes: u64,
) -> Result<Vec<String>, StorageError> {
    let file = fs::File::open(path).map_err(|error| {
        StorageError::InvalidInput(format!(
            "{kind} requires readable text at {}: {error}",
            path.display()
        ))
    })?;
    let mut reader = BufReader::new(file);
    let mut lines = Vec::new();
    let mut total_bytes = 0_u64;
    let mut line = String::new();
    loop {
        line.clear();
        let count = reader.read_line(&mut line).map_err(|error| {
            StorageError::InvalidInput(format!(
                "{kind} requires UTF-8 text at {}: {error}",
                path.display()
            ))
        })?;
        if count == 0 {
            break;
        }
        total_bytes = total_bytes.saturating_add(count as u64);
        if total_bytes > max_bytes {
            return Err(StorageError::InvalidInput(format!(
                "{kind} exceeds {max_bytes} byte cap at {}",
                path.display()
            )));
        }
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            lines.push(trimmed.to_string());
        }
    }
    Ok(lines)
}

fn validate_declared_inputs(
    inputs: &BTreeMap<String, ResolvedInput>,
    ports: &BTreeMap<String, crate::storage::ToolPortSpec>,
) -> Result<(), StorageError> {
    for (name, input) in inputs {
        if let Some(port) = ports.get(name) {
            validate_port_file("input", name, &input.path, port)?;
        }
    }
    validate_sample_id_consistency(inputs, ports)?;
    Ok(())
}

fn validate_declared_outputs(
    outputs: &OutputPaths,
    ports: &BTreeMap<String, crate::storage::ToolPortSpec>,
) -> Result<(), StorageError> {
    for (name, path) in outputs.as_map() {
        if let Some(port) = ports.get(name) {
            validate_port_file("output", name, path, port)?;
        }
    }
    Ok(())
}

fn validate_port_file(
    direction: &str,
    name: &str,
    path: &Path,
    port: &crate::storage::ToolPortSpec,
) -> Result<(), StorageError> {
    if port.min_rows.is_none()
        && port.required_columns.is_empty()
        && port.sample_id_column.is_none()
    {
        return Ok(());
    }

    let lines = read_trimmed_nonempty_lines_with_byte_cap(
        &format!("{direction} {name} validator"),
        path,
        MAX_RUNTIME_ARTIFACT_TEXT_BYTES,
    )?;

    let has_header_validators =
        !port.required_columns.is_empty() || port.sample_id_column.is_some();
    let data_rows = if !has_header_validators {
        lines.len()
    } else {
        let header = lines.first().ok_or_else(|| {
            StorageError::InvalidInput(format!(
                "{direction} {name} is empty and cannot satisfy required_columns"
            ))
        })?;
        let delimiter = delimiter_for_port(&port.type_name, header);
        let columns = header
            .split(delimiter)
            .map(str::trim)
            .collect::<BTreeSet<_>>();
        for required in &port.required_columns {
            if !columns.contains(required.as_str()) {
                return Err(StorageError::InvalidInput(format!(
                    "{direction} {name} missing required column {required}"
                )));
            }
        }
        if let Some(sample_id_column) = port.sample_id_column.as_deref() {
            if !columns.contains(sample_id_column) {
                return Err(StorageError::InvalidInput(format!(
                    "{direction} {name} missing sample_id_column {sample_id_column}"
                )));
            }
        }
        lines.len().saturating_sub(1)
    };

    if let Some(min_rows) = port.min_rows {
        if data_rows < min_rows {
            return Err(StorageError::InvalidInput(format!(
                "{direction} {name} has {data_rows} rows, expected at least {min_rows}"
            )));
        }
    }

    Ok(())
}

fn validate_sample_id_consistency(
    inputs: &BTreeMap<String, ResolvedInput>,
    ports: &BTreeMap<String, crate::storage::ToolPortSpec>,
) -> Result<(), StorageError> {
    let mut sample_sets: Vec<(String, BTreeSet<String>)> = Vec::new();
    for (name, input) in inputs {
        let Some(port) = ports.get(name) else {
            continue;
        };
        let Some(sample_id_column) = port.sample_id_column.as_deref() else {
            continue;
        };
        let ids = sample_ids_for_input(name, &input.path, port, sample_id_column)?;
        sample_sets.push((name.clone(), ids));
    }

    let Some((reference_name, reference_ids)) = sample_sets.first() else {
        return Ok(());
    };
    for (name, ids) in sample_sets.iter().skip(1) {
        if ids != reference_ids {
            let missing = reference_ids.difference(ids).cloned().collect::<Vec<_>>();
            let extra = ids.difference(reference_ids).cloned().collect::<Vec<_>>();
            return Err(StorageError::InvalidInput(format!(
                "input {name} sample ids differ from {reference_name}: missing [{}], extra [{}]",
                preview_values(&missing),
                preview_values(&extra)
            )));
        }
    }
    Ok(())
}

fn sample_ids_for_input(
    name: &str,
    path: &Path,
    port: &crate::storage::ToolPortSpec,
    sample_id_column: &str,
) -> Result<BTreeSet<String>, StorageError> {
    let lines = read_trimmed_nonempty_lines_with_byte_cap(
        &format!("input {name} sample_id_column validator"),
        path,
        MAX_RUNTIME_ARTIFACT_TEXT_BYTES,
    )?;
    let header = lines.first().ok_or_else(|| {
        StorageError::InvalidInput(format!(
            "input {name} is empty and cannot satisfy sample_id_column"
        ))
    })?;
    let delimiter = delimiter_for_port(&port.type_name, header);
    let columns = header.split(delimiter).map(str::trim).collect::<Vec<_>>();
    let Some(column_index) = columns
        .iter()
        .position(|column| *column == sample_id_column)
    else {
        return Err(StorageError::InvalidInput(format!(
            "input {name} missing sample_id_column {sample_id_column}"
        )));
    };

    let mut ids = BTreeSet::new();
    for (row_index, line) in lines.iter().skip(1).enumerate() {
        let value = line
            .split(delimiter)
            .nth(column_index)
            .map(str::trim)
            .unwrap_or_default();
        if value.is_empty() {
            return Err(StorageError::InvalidInput(format!(
                "input {name} has empty sample id in row {}",
                row_index + 2
            )));
        }
        if !ids.insert(value.to_string()) {
            return Err(StorageError::InvalidInput(format!(
                "input {name} has duplicate sample id {value}"
            )));
        }
    }
    if ids.is_empty() {
        return Err(StorageError::InvalidInput(format!(
            "input {name} sample_id_column {sample_id_column} has no ids"
        )));
    }
    Ok(ids)
}

fn preview_values(values: &[String]) -> String {
    let preview = values
        .iter()
        .take(5)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(",");
    if values.len() > 5 {
        format!("{preview},...")
    } else {
        preview
    }
}

fn delimiter_for_port(type_name: &str, header: &str) -> char {
    if type_name.to_ascii_lowercase().contains("csv")
        || (header.contains(',') && !header.contains('\t'))
    {
        ','
    } else {
        '\t'
    }
}

fn env_vars(
    inputs: &BTreeMap<String, PathBuf>,
    params: &BTreeMap<String, String>,
    outputs: &BTreeMap<String, PathBuf>,
) -> Vec<(String, String)> {
    let mut vars = Vec::new();
    for (name, path) in inputs {
        vars.push((
            format!("AGENTFLOW_INPUT_{}", env_key(name)),
            path.display().to_string(),
        ));
    }
    for (name, value) in params {
        vars.push((format!("AGENTFLOW_PARAM_{}", env_key(name)), value.clone()));
    }
    for (name, path) in outputs {
        vars.push((
            format!("AGENTFLOW_OUTPUT_{}", env_key(name)),
            path.display().to_string(),
        ));
    }
    vars
}

fn agentflow_env_names(step_env_vars: &[(String, String)]) -> Vec<String> {
    let mut names = vec![
        "AGENTFLOW_WORKDIR".to_string(),
        "AGENTFLOW_INPUTS_JSON".to_string(),
        "AGENTFLOW_PARAMS_JSON".to_string(),
        "AGENTFLOW_OUTPUTS_JSON".to_string(),
    ];
    names.extend(
        step_env_vars
            .iter()
            .filter(|(name, _)| name.starts_with("AGENTFLOW_"))
            .map(|(name, _)| name.clone()),
    );
    names
}

fn singularity_env_vars(
    config: &RunConfig,
    fixed_env_vars: &[(String, String)],
    step_env_vars: &[(String, String)],
) -> Vec<(String, String)> {
    let is_singularity = config
        .container_engine
        .as_ref()
        .map(|selection| selection.kind == ContainerEngineKind::Singularity)
        .unwrap_or(false);
    if !is_singularity {
        return Vec::new();
    }

    fixed_env_vars
        .iter()
        .chain(step_env_vars.iter())
        .filter(|(name, _)| name.starts_with("AGENTFLOW_"))
        .map(|(name, value)| (format!("SINGULARITYENV_{name}"), value.clone()))
        .collect()
}

fn parse_json_map(input: &str) -> Result<BTreeMap<String, String>, StorageError> {
    serde_json::from_str(input)
        .map_err(|err| StorageError::InvalidInput(format!("cannot parse map: {input}: {err}")))
}

fn path_map_json(map: &BTreeMap<String, PathBuf>) -> String {
    let path_map = map
        .iter()
        .map(|(key, value)| (key.clone(), value.display().to_string()))
        .collect::<BTreeMap<_, _>>();
    serde_json::to_string(&path_map).expect("path map serializes to JSON")
}

fn string_map_json(map: &BTreeMap<String, String>) -> String {
    serde_json::to_string(map).expect("string map serializes to JSON")
}

#[cfg(test)]
fn string_array_json(values: &[String]) -> String {
    serde_json::to_string(values).expect("string array serializes to JSON")
}

fn shell_display(command: &[String]) -> String {
    command
        .iter()
        .map(|arg| {
            if arg
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '-' | '.'))
            {
                arg.clone()
            } else {
                format!("'{}'", arg.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn sanitize_path_part(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        "output".to_string()
    } else {
        sanitized
    }
}

fn env_key(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

use crate::storage::now_unix_nanos;

#[cfg(test)]
mod tests {
    use super::schedule::{RuleBasedStepScheduler, StepScheduler};
    use super::*;
    use crate::storage::{
        ArtifactImportMode, ArtifactImportRequest, FlowDraft, ProjectStore, StoredFlowEdge,
        ToolSpec,
    };
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn temp_project_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentflow-runtime-{test_name}-{}-{}",
            std::process::id(),
            now_unix_nanos()
        ))
    }

    fn test_exec_context<'a>(
        staged_inputs: &'a BTreeMap<String, PathBuf>,
    ) -> backend::ExecContext<'a> {
        backend::ExecContext {
            workdir: Path::new("/tmp/agentflow-test-workdir"),
            staged_inputs,
            output_dir: Path::new("/tmp/agentflow-test-workdir/outputs"),
            env_names: &[],
            container_engine: None,
        }
    }

    fn write_script(path: &Path) -> PathBuf {
        let script_path = path.join("marker_tool.sh");
        fs::write(
            &script_path,
            r#"if [ -n "$AGENTFLOW_OUTPUT_MARKER_REPORT" ]; then
  cat "$AGENTFLOW_INPUT_EXPRESSION_TABLE" >/dev/null
  cat "$AGENTFLOW_INPUT_SURVIVAL_TABLE" >/dev/null
  printf '# Marker report\nGene: %s\nscore: 0.61\n' "$AGENTFLOW_PARAM_GENE" > "$AGENTFLOW_OUTPUT_MARKER_REPORT"
  echo "scan ok"
fi
if [ -n "$AGENTFLOW_OUTPUT_FINAL_REPORT" ]; then
  cat "$AGENTFLOW_INPUT_UPSTREAM_REPORT" > "$AGENTFLOW_OUTPUT_FINAL_REPORT"
  printf '\nfinalized\n' >> "$AGENTFLOW_OUTPUT_FINAL_REPORT"
  echo "finalize ok"
fi
"#,
        )
        .unwrap();
        script_path
    }

    fn write_note_script(path: &Path) -> PathBuf {
        let script_path = path.join("note_tool.sh");
        fs::write(
            &script_path,
            r#"printf '%s\n' "$AGENTFLOW_PARAM_LABEL" > "$AGENTFLOW_OUTPUT_NOTE"
echo "$AGENTFLOW_PARAM_LABEL"
"#,
        )
        .unwrap();
        script_path
    }

    fn stored_step(local_id: &str) -> StoredFlowStep {
        StoredFlowStep {
            id: format!("step:schedule_test/{local_id}"),
            local_id: local_id.to_string(),
            tool_ref: Some("schedule/noop".to_string()),
            step_type: "tool".to_string(),
            status: "ready".to_string(),
            reason: None,
            params_json: "{}".to_string(),
            inputs_json: "{}".to_string(),
            outputs_json: "{}".to_string(),
        }
    }

    fn stored_edge(from: &str, to: &str) -> StoredFlowEdge {
        StoredFlowEdge {
            from_step_id: format!("step:schedule_test/{from}"),
            to_step_id: format!("step:schedule_test/{to}"),
            from_local_id: from.to_string(),
            to_local_id: to.to_string(),
            edge_type: "needs".to_string(),
        }
    }

    fn make_executable(path: &Path) {
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).unwrap();
        }
    }

    fn write_fake_environment_runner(path: &Path) -> PathBuf {
        let runner_path = path.join("fake_micromamba.sh");
        fs::write(
            &runner_path,
            r#"#!/bin/sh
if [ "$1" = "env" ] && [ "$2" = "update" ]; then
  echo "fake env update $*"
  exit 0
fi
if [ "$1" = "env" ] && [ "$2" = "export" ]; then
  printf 'name: af-test\ndependencies:\n  - python=3.11\n  - pandas\n  - scanpy\n'
  exit 0
fi
if [ "$1" != "run" ]; then
  echo "expected run, env update, or env export subcommand" >&2
  exit 91
fi
shift
while [ "$#" -gt 0 ]; do
  case "$1" in
    --name|--prefix)
      shift 2
      ;;
    --no-capture-output)
      shift
      ;;
    *)
      break
      ;;
  esac
done
exec "$@"
"#,
        )
        .unwrap();
        make_executable(&runner_path);
        runner_path
    }

    fn write_pythonpath_echo_script(path: &Path) -> PathBuf {
        let script_path = path.join("pythonpath_echo.sh");
        fs::write(
            &script_path,
            r#"#!/bin/sh
printf '%s\n' "$PYTHONPATH" > "$AGENTFLOW_OUTPUT_RESULT"
printf '%s\n' "$PYTHONPATH"
"#,
        )
        .unwrap();
        make_executable(&script_path);
        script_path
    }

    fn run_pythonpath_echo_flow(
        namespace: &str,
        test_name: &str,
    ) -> (PathBuf, FlowRunSummary, RunLogs, PathBuf) {
        let path = temp_project_path(test_name);
        fs::create_dir_all(&path).unwrap();
        let existing_pythonpath = path.join("existing-pythonpath");
        fs::create_dir_all(&existing_pythonpath).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();
        let script_path = write_pythonpath_echo_script(&path);
        let script = script_path.display();
        let pythonpath = existing_pythonpath.display();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: {namespace}
name: pythonpath_echo
version: 0.1.0
maturity: exploratory
description: Echo PYTHONPATH for guard testing
outputs:
  result:
    type: Text
runtime:
  backend: local
  command:
    - /usr/bin/env
    - PYTHONPATH={pythonpath}
    - /bin/sh
    - {script}
"#
            ),
        );

        let flow = FlowDraft::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.flow.v0
id: pythonpath_demo
name: Pythonpath demo
steps:
  - id: echo
    tool: {namespace}/pythonpath_echo
    needs: []
    outputs:
      result: result
"#
        ))
        .unwrap();
        store.approve_flow(flow, None).unwrap();
        let summary = store.run_flow("pythonpath_demo").unwrap();
        let logs = store.read_logs(&summary.attempts[0].attempt_id).unwrap();

        (path, summary, logs, existing_pythonpath)
    }

    fn register_tool(store: &ProjectStore, source: String) {
        let spec = ToolSpec::from_simple_yaml(&source).unwrap();
        store.register_tool(spec).unwrap();
    }

    fn import_artifact(store: &ProjectStore, source_path: PathBuf) -> String {
        store
            .import_artifact(ArtifactImportRequest {
                source_path,
                artifact_type: "TSV".to_string(),
                mode: ArtifactImportMode::Reference,
            })
            .unwrap()
            .summary
            .id
    }

    fn run_schedule_flow(test_name: &str, serial: bool) -> (PathBuf, FlowRunSummary) {
        run_schedule_flow_with(test_name, serial, 0)
    }

    // A fan-out flow with a failing root `bad` (declared first, so it runs first)
    // and an independent succeeding root `good` -> `good_tail`; `bad_tail` depends
    // on the failed `bad`. Runs in a fresh project with the given keep_going.
    fn run_keep_going_flow(test_name: &str, keep_going: bool) -> FlowRunSummary {
        run_keep_going_flow_with(test_name, keep_going, 0)
    }

    fn run_keep_going_flow_with(
        test_name: &str,
        keep_going: bool,
        max_parallel: usize,
    ) -> FlowRunSummary {
        let path = temp_project_path(test_name);
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Keep Going Demo")).unwrap();
        let ok = write_note_script(&path);
        let fail = path.join("fail_tool.sh");
        fs::write(&fail, "echo boom 1>&2\nexit 1\n").unwrap();

        register_tool(
            &store,
            format!(
                "schema_version: agentflow.tool.v0\nnamespace: kg\nname: emit_note\nversion: 0.1.0\nmaturity: wrapped\ndescription: Emit a note\nparams:\n  label:\n    type: string\n    required: true\noutputs:\n  note:\n    type: Text\nruntime:\n  backend: local\n  command:\n    - /bin/sh\n    - {}\n",
                ok.display()
            ),
        );
        register_tool(
            &store,
            format!(
                "schema_version: agentflow.tool.v0\nnamespace: kg\nname: fail\nversion: 0.1.0\nmaturity: wrapped\ndescription: Always fails\noutputs:\n  note:\n    type: Text\nruntime:\n  backend: local\n  command:\n    - /bin/sh\n    - {}\n",
                fail.display()
            ),
        );

        let flow = FlowDraft::from_simple_yaml(
            "schema_version: agentflow.flow.v0\nid: kg_demo\nname: Keep going demo\nsteps:\n  - id: bad\n    tool: kg/fail\n    needs: []\n    outputs:\n      note: bad_note\n  - id: good\n    tool: kg/emit_note\n    needs: []\n    params:\n      label: good\n    outputs:\n      note: good_note\n  - id: good_tail\n    tool: kg/emit_note\n    needs: [good]\n    params:\n      label: good_tail\n    outputs:\n      note: good_tail_note\n  - id: bad_tail\n    tool: kg/emit_note\n    needs: [bad]\n    params:\n      label: bad_tail\n    outputs:\n      note: bad_tail_note\n",
        )
        .unwrap();
        store.approve_flow(flow, None).unwrap();
        let summary = store
            .run_flow_with(
                "kg_demo",
                &RunConfig {
                    keep_going,
                    max_parallel,
                    ..Default::default()
                },
            )
            .unwrap();
        let _ = fs::remove_dir_all(&path);
        summary
    }

    #[test]
    fn keep_going_runs_independent_steps_after_a_failure() {
        // Fail-fast (default): stop at the first failure — `bad` runs and fails,
        // nothing else is attempted.
        let fail_fast = run_keep_going_flow("keep-going-off", false);
        assert_eq!(fail_fast.failed_steps, 1);
        assert_eq!(fail_fast.completed_steps, 0);
        assert_eq!(fail_fast.attempts.len(), 1);

        // Keep-going: `bad` fails but the independent `good` -> `good_tail` branch
        // still runs; `bad_tail` (dependent on the failed `bad`) is skipped.
        let keep_going = run_keep_going_flow("keep-going-on", true);
        assert_eq!(keep_going.failed_steps, 1);
        assert_eq!(keep_going.completed_steps, 2);
        let ran: Vec<&str> = keep_going
            .attempts
            .iter()
            .map(|attempt| attempt.step_id.as_str())
            .collect();
        assert!(ran.iter().any(|id| id.ends_with("/good")));
        assert!(ran.iter().any(|id| id.ends_with("/good_tail")));
        assert!(
            !ran.iter().any(|id| id.ends_with("/bad_tail")),
            "dependent of a failed step must be skipped: {ran:?}"
        );

        // Keep-going holds on the parallel path too (run_ready_wave_parallel does
        // not stop the wave on a batch failure when keep_going is set).
        let parallel = run_keep_going_flow_with("keep-going-parallel", true, 4);
        assert_eq!(parallel.failed_steps, 1);
        assert_eq!(parallel.completed_steps, 2);
        assert!(!parallel
            .attempts
            .iter()
            .any(|attempt| attempt.step_id.ends_with("/bad_tail")));
    }

    fn run_schedule_flow_with(
        test_name: &str,
        serial: bool,
        max_parallel: usize,
    ) -> (PathBuf, FlowRunSummary) {
        let path = temp_project_path(test_name);
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Schedule Demo")).unwrap();
        let script_path = write_note_script(&path);
        let command = script_path.display();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: schedule
name: emit_note
version: 0.1.0
maturity: wrapped
description: Emit a deterministic note
params:
  label:
    type: string
    required: true
outputs:
  note:
    type: Text
runtime:
  backend: local
  command:
    - /bin/sh
    - {command}
"#
            ),
        );

        let wide_needs = if serial { "[narrow]" } else { "[]" };
        let flow = FlowDraft::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.flow.v0
id: schedule_demo
name: Schedule demo
steps:
  - id: narrow
    tool: schedule/emit_note
    needs: []
    params:
      label: narrow
    outputs:
      note: narrow_note
  - id: wide
    tool: schedule/emit_note
    needs: {wide_needs}
    params:
      label: wide
    outputs:
      note: wide_note
  - id: join
    tool: schedule/emit_note
    needs: [narrow, wide]
    params:
      label: join
    outputs:
      note: join_note
  - id: wide_tail
    tool: schedule/emit_note
    needs: [wide]
    params:
      label: wide_tail
    outputs:
      note: wide_tail_note
"#
        ))
        .unwrap();
        store.approve_flow(flow, None).unwrap();
        let summary = store
            .run_flow_with(
                "schedule_demo",
                &RunConfig {
                    max_parallel,
                    ..Default::default()
                },
            )
            .unwrap();
        (path, summary)
    }

    #[test]
    fn parallel_execution_matches_serial_outputs_and_lineage() {
        // Serial baseline: the fan-out flow (serial=false makes `wide` ready
        // alongside `narrow`), executed sequentially (max_parallel = 0).
        let (serial_path, serial_summary) = run_schedule_flow("parallel-baseline", false);
        let serial_outputs = computed_texts_by_step(&serial_path, "schedule_demo");

        // Same flow, executed with up to four subprocesses overlapping.
        let (parallel_path, parallel_summary) = run_schedule_flow_with("parallel-real", false, 4);
        let parallel_outputs = computed_texts_by_step(&parallel_path, "schedule_demo");

        assert_eq!(serial_summary.completed_steps, 4);
        assert_eq!(serial_summary.failed_steps, 0);
        assert_eq!(parallel_summary.completed_steps, 4);
        assert_eq!(parallel_summary.failed_steps, 0);
        // The parallel run produces byte-identical computed outputs for every step.
        assert_eq!(parallel_outputs, serial_outputs);

        let _ = fs::remove_dir_all(serial_path);
        let _ = fs::remove_dir_all(parallel_path);
    }

    fn computed_texts_by_step(path: &Path, flow_id: &str) -> BTreeMap<String, String> {
        let store = ProjectStore::open(path).unwrap();
        let mut texts = BTreeMap::new();
        for artifact in store.list_artifacts().unwrap() {
            if artifact.kind != "computed" {
                continue;
            }
            let Some(source_step_id) = artifact.source_step_id.as_deref() else {
                continue;
            };
            let Some(local_id) = source_step_id.strip_prefix(&format!("step:{flow_id}/")) else {
                continue;
            };
            texts.insert(
                local_id.to_string(),
                fs::read_to_string(artifact.path).unwrap(),
            );
        }
        texts
    }

    #[test]
    fn rule_based_step_scheduler_orders_by_downstream_count_then_declaration_order() {
        let ready = vec![
            stored_step("single"),
            stored_step("fanout"),
            stored_step("tie_first"),
            stored_step("tie_second"),
        ];
        let edges = vec![
            stored_edge("single", "single_child"),
            stored_edge("fanout", "fanout_a"),
            stored_edge("fanout", "fanout_b"),
            stored_edge("tie_first", "tie_first_child"),
            stored_edge("tie_second", "tie_second_child"),
        ];

        let ordered = RuleBasedStepScheduler.order(ready, &edges);

        assert_eq!(
            ordered
                .iter()
                .map(|step| step.local_id.as_str())
                .collect::<Vec<_>>(),
            ["fanout", "single", "tie_first", "tie_second"]
        );
    }

    #[test]
    fn rule_based_step_scheduler_keeps_single_ready_step_unchanged() {
        let ready = vec![stored_step("only")];
        let edges = vec![stored_edge("only", "child")];

        let ordered = RuleBasedStepScheduler.order(ready.clone(), &edges);

        assert_eq!(ordered, ready);
    }

    #[test]
    fn run_flow_schedules_parallel_ready_steps_deterministically_without_changing_outputs() {
        let (serial_path, serial_summary) = run_schedule_flow("scheduler-serial", true);
        let serial_outputs = computed_texts_by_step(&serial_path, "schedule_demo");
        let (parallel_path, parallel_summary) = run_schedule_flow("scheduler-parallel", false);
        let parallel_outputs = computed_texts_by_step(&parallel_path, "schedule_demo");

        assert_eq!(serial_summary.completed_steps, 4);
        assert_eq!(serial_summary.failed_steps, 0);
        assert_eq!(parallel_summary.completed_steps, 4);
        assert_eq!(parallel_summary.failed_steps, 0);
        assert_eq!(
            parallel_summary
                .attempts
                .iter()
                .map(|attempt| attempt.step_id.as_str())
                .collect::<Vec<_>>(),
            [
                "step:schedule_demo/wide",
                "step:schedule_demo/narrow",
                "step:schedule_demo/join",
                "step:schedule_demo/wide_tail"
            ]
        );
        assert_eq!(parallel_outputs, serial_outputs);
        assert_eq!(parallel_outputs["narrow"], "narrow\n");
        assert_eq!(parallel_outputs["wide"], "wide\n");
        assert_eq!(parallel_outputs["join"], "join\n");
        assert_eq!(parallel_outputs["wide_tail"], "wide_tail\n");

        let _ = fs::remove_dir_all(serial_path);
        let _ = fs::remove_dir_all(parallel_path);
    }

    #[test]
    fn runtime_json_map_parser_handles_punctuation_inside_strings() {
        let parsed =
            parse_json_map(r#"{"gene":"TP53,EGFR:ALK","label":"quoted \"value\""}"#).unwrap();
        assert_eq!(parsed["gene"], "TP53,EGFR:ALK");
        assert_eq!(parsed["label"], "quoted \"value\"");
    }

    #[test]
    fn capped_stream_capture_marks_truncation() {
        let captured =
            capture_stream_with_byte_cap(std::io::Cursor::new("abcdef"), 4, "stdout").unwrap();

        assert_eq!(
            String::from_utf8(captured).unwrap(),
            "abcd\n[agentflow] stdout truncated after 4 bytes\n"
        );
    }

    #[test]
    fn validator_text_reader_rejects_file_over_injected_byte_cap() {
        let path = temp_project_path("validator-byte-cap");
        fs::create_dir_all(&path).unwrap();
        let table = path.join("table.tsv");
        fs::write(&table, "sample\tvalue\nA\t1\n").unwrap();

        let error = read_trimmed_nonempty_lines_with_byte_cap("input table validator", &table, 8)
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("input table validator exceeds 8 byte cap"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn synth_namespace_runtime_prepends_python_egress_guard_to_pythonpath() {
        let (path, summary, logs, existing_pythonpath) =
            run_pythonpath_echo_flow("synth", "synth-runtime-egress-guard");
        let guard_dir = summary.attempts[0].workdir.join("python-egress-guard");
        let pythonpath = std::ffi::OsString::from(logs.stdout.trim());
        let paths = std::env::split_paths(&pythonpath).collect::<Vec<_>>();

        assert_eq!(paths.first(), Some(&guard_dir));
        assert_eq!(paths.get(1), Some(&existing_pythonpath));
        assert_eq!(
            fs::read_to_string(guard_dir.join("sitecustomize.py")).unwrap(),
            PYTHON_EGRESS_GUARD_SITECUSTOMIZE
        );

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn non_synth_runtime_does_not_inject_python_egress_guard() {
        let (path, summary, logs, existing_pythonpath) =
            run_pythonpath_echo_flow("marker", "local-runtime-no-egress-guard");
        let guard_dir = summary.attempts[0].workdir.join("python-egress-guard");
        let pythonpath = std::ffi::OsString::from(logs.stdout.trim());
        let paths = std::env::split_paths(&pythonpath).collect::<Vec<_>>();

        assert_eq!(paths.first(), Some(&existing_pythonpath));
        assert!(!guard_dir.exists());

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn runtime_json_helpers_are_exact_byte_and_serde_readable() {
        let mut map = BTreeMap::new();
        map.insert("gene".to_string(), "TP53".to_string());
        map.insert("label".to_string(), "quoted \"value\"".to_string());
        assert_eq!(
            string_map_json(&map),
            "{\"gene\":\"TP53\",\"label\":\"quoted \\\"value\\\"\"}"
        );
        assert_eq!(parse_json_map(&string_map_json(&map)).unwrap(), map);

        let mut paths = BTreeMap::new();
        paths.insert("report".to_string(), PathBuf::from("/tmp/report.md"));
        assert_eq!(path_map_json(&paths), "{\"report\":\"/tmp/report.md\"}");
        assert_eq!(
            string_array_json(&["/bin/echo".to_string(), "hello world".to_string()]),
            "[\"/bin/echo\",\"hello world\"]"
        );

        let runtime = ToolRuntimeSpec {
            backend: "local".to_string(),
            command: vec!["/bin/echo".to_string(), "hello world".to_string()],
            timeout_seconds: Some(5),
            env_name: None,
            env_prefix: None,
            env_file: None,
            runner: None,
            image: None,
        };
        let json = runtime_config_json(&runtime).unwrap();
        assert_eq!(
            json,
            "{\"backend\":\"local\",\"command\":[\"/bin/echo\",\"hello world\"],\"timeout_seconds\":5,\"env_name\":null,\"env_prefix\":null,\"env_file\":null,\"env_file_hash\":null,\"runner\":null}"
        );
        let payload: RuntimeConfigJson = serde_json::from_str(&json).unwrap();
        assert_eq!(payload.command, ["/bin/echo", "hello world"]);
    }

    #[test]
    fn local_runtime_command_builder_keeps_argv_unchanged() {
        let runtime = ToolRuntimeSpec {
            backend: "local".to_string(),
            command: vec!["/bin/echo".to_string(), "hello world".to_string()],
            timeout_seconds: Some(5),
            env_name: None,
            env_prefix: None,
            env_file: None,
            runner: None,
            image: None,
        };
        let staged_inputs = BTreeMap::new();
        let ctx = test_exec_context(&staged_inputs);

        assert_eq!(
            prepare_runtime_command(&runtime, &ctx).unwrap().argv(),
            vec!["/bin/echo".to_string(), "hello world".to_string()]
        );
    }

    #[test]
    fn local_runtime_command_ignores_exec_context() {
        let runtime = ToolRuntimeSpec {
            backend: "local".to_string(),
            command: vec!["/bin/echo".to_string(), "hello world".to_string()],
            timeout_seconds: Some(5),
            env_name: None,
            env_prefix: None,
            env_file: None,
            runner: None,
            image: None,
        };
        let first_inputs = BTreeMap::new();
        let first_ctx = backend::ExecContext {
            workdir: Path::new("/tmp/af-work-a"),
            staged_inputs: &first_inputs,
            output_dir: Path::new("/tmp/af-work-a/outputs"),
            env_names: &[],
            container_engine: None,
        };
        let mut second_inputs = BTreeMap::new();
        second_inputs.insert(
            "reads".to_string(),
            PathBuf::from("/tmp/af-work-b/inputs/reads/in.fastq"),
        );
        let second_ctx = backend::ExecContext {
            workdir: Path::new("/tmp/af-work-b"),
            staged_inputs: &second_inputs,
            output_dir: Path::new("/tmp/af-work-b/outputs"),
            env_names: &[],
            container_engine: None,
        };

        assert_eq!(
            prepare_runtime_command(&runtime, &first_ctx)
                .unwrap()
                .argv(),
            prepare_runtime_command(&runtime, &second_ctx)
                .unwrap()
                .argv()
        );
    }

    fn env_runtime(backend: &str) -> ToolRuntimeSpec {
        ToolRuntimeSpec {
            backend: backend.to_string(),
            command: vec!["python".to_string(), "tool.py".to_string()],
            timeout_seconds: None,
            env_name: Some("envA".to_string()),
            env_prefix: None,
            env_file: None,
            runner: Some("/opt/micromamba".to_string()),
            image: None,
        }
    }

    #[test]
    fn container_runtime_command_builds_hard_isolation_argv_and_preserves_tool_command_suffix() {
        let runtime = ToolRuntimeSpec {
            backend: "container".to_string(),
            command: vec![
                "python".to_string(),
                "tool.py".to_string(),
                "--mode".to_string(),
                "strict".to_string(),
            ],
            timeout_seconds: None,
            env_name: None,
            env_prefix: None,
            env_file: None,
            runner: Some("/usr/bin/docker".to_string()),
            image: Some("ghcr.io/acme/tool@sha256:0123456789abcdef".to_string()),
        };
        let staged_inputs = BTreeMap::new();
        let env_names = vec![
            "AGENTFLOW_WORKDIR".to_string(),
            "AGENTFLOW_INPUT_READS".to_string(),
            "AGENTFLOW_PARAMS_JSON".to_string(),
            "AGENTFLOW_OUTPUT_REPORT".to_string(),
        ];
        let ctx = backend::ExecContext {
            workdir: Path::new("/tmp/af-step-work"),
            staged_inputs: &staged_inputs,
            output_dir: Path::new("/tmp/af-step-work/outputs"),
            env_names: &env_names,
            container_engine: None,
        };

        let prepared = prepare_runtime_command(&runtime, &ctx).unwrap();

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
            &prepared.args[prepared.args.len() - runtime.command.len()..],
            runtime.command.as_slice()
        );
        assert!(backend::backend_for("container").is_some());
    }

    #[test]
    fn container_engine_selection_does_not_change_runtime_config_or_cache_key() {
        let runtime = ToolRuntimeSpec {
            backend: "container".to_string(),
            command: vec!["python".to_string(), "run.py".to_string()],
            timeout_seconds: None,
            env_name: None,
            env_prefix: None,
            env_file: None,
            runner: Some("/usr/bin/docker".to_string()),
            image: Some("ghcr.io/acme/tool@sha256:0123456789abcdef".to_string()),
        };
        let docker_selection = ContainerEngineSelection {
            kind: ContainerEngineKind::Docker,
            runner: Some(PathBuf::from("/custom/docker")),
        };
        let podman_selection = ContainerEngineSelection {
            kind: ContainerEngineKind::Podman,
            runner: Some(PathBuf::from("/custom/podman")),
        };
        let singularity_selection = ContainerEngineSelection {
            kind: ContainerEngineKind::Singularity,
            runner: Some(PathBuf::from("/custom/apptainer")),
        };
        let staged_inputs = BTreeMap::new();
        let docker_ctx = backend::ExecContext {
            workdir: Path::new("/tmp/af-step-work"),
            staged_inputs: &staged_inputs,
            output_dir: Path::new("/tmp/af-step-work/outputs"),
            env_names: &[],
            container_engine: Some(&docker_selection),
        };
        let podman_ctx = backend::ExecContext {
            workdir: Path::new("/tmp/af-step-work"),
            staged_inputs: &staged_inputs,
            output_dir: Path::new("/tmp/af-step-work/outputs"),
            env_names: &[],
            container_engine: Some(&podman_selection),
        };
        let singularity_ctx = backend::ExecContext {
            workdir: Path::new("/tmp/af-step-work"),
            staged_inputs: &staged_inputs,
            output_dir: Path::new("/tmp/af-step-work/outputs"),
            env_names: &[],
            container_engine: Some(&singularity_selection),
        };

        let docker_command = prepare_runtime_command(&runtime, &docker_ctx).unwrap();
        let podman_command = prepare_runtime_command(&runtime, &podman_ctx).unwrap();
        let singularity_command = prepare_runtime_command(&runtime, &singularity_ctx).unwrap();
        let docker_runtime_json = runtime_config_json(&runtime).unwrap();
        let singularity_runtime_json = runtime_config_json(&runtime).unwrap();
        let runtime_hash = stable_hash(&docker_runtime_json);
        let docker_cache_key = compute_cache_key(
            "analysis/tool",
            "0.1.0",
            "{}",
            &stable_hash("{}"),
            &runtime_hash,
        );
        let podman_cache_key = compute_cache_key(
            "analysis/tool",
            "0.1.0",
            "{}",
            &stable_hash("{}"),
            &runtime_hash,
        );
        let singularity_cache_key = compute_cache_key(
            "analysis/tool",
            "0.1.0",
            "{}",
            &stable_hash("{}"),
            &runtime_hash,
        );

        assert_eq!(docker_command.executable, "/custom/docker");
        assert_eq!(podman_command.executable, "/custom/podman");
        assert_eq!(singularity_command.executable, "/custom/apptainer");
        assert_eq!(docker_command.args, podman_command.args);
        assert_ne!(docker_command.args, singularity_command.args);
        assert_eq!(docker_runtime_json, singularity_runtime_json);
        assert!(!docker_runtime_json.contains("Docker"));
        assert!(!docker_runtime_json.contains("Podman"));
        assert!(!docker_runtime_json.contains("Singularity"));
        assert_eq!(docker_cache_key, podman_cache_key);
        assert_eq!(docker_cache_key, singularity_cache_key);
    }

    #[test]
    fn singularity_env_forwarding_adds_prefixed_agentflow_vars_only_for_singularity() {
        let step_env_vars = vec![
            (
                "AGENTFLOW_INPUT_READS".to_string(),
                "/tmp/af-step-work/inputs/reads/in.fastq".to_string(),
            ),
            ("AGENTFLOW_PARAM_MODE".to_string(), "strict".to_string()),
            (
                "AGENTFLOW_OUTPUT_REPORT".to_string(),
                "/tmp/af-step-work/outputs/report".to_string(),
            ),
        ];
        let fixed_env_vars = vec![
            (
                "AGENTFLOW_WORKDIR".to_string(),
                "/tmp/af-step-work".to_string(),
            ),
            (
                "AGENTFLOW_INPUTS_JSON".to_string(),
                "/tmp/af-step-work/inputs.json".to_string(),
            ),
            (
                "AGENTFLOW_PARAMS_JSON".to_string(),
                "/tmp/af-step-work/params.json".to_string(),
            ),
            (
                "AGENTFLOW_OUTPUTS_JSON".to_string(),
                "/tmp/af-step-work/outputs.json".to_string(),
            ),
        ];
        let singularity_config = RunConfig {
            container_engine: Some(ContainerEngineSelection {
                kind: ContainerEngineKind::Singularity,
                runner: Some(PathBuf::from("/usr/bin/apptainer")),
            }),
            ..Default::default()
        };
        let docker_config = RunConfig {
            container_engine: Some(ContainerEngineSelection {
                kind: ContainerEngineKind::Docker,
                runner: Some(PathBuf::from("/usr/bin/docker")),
            }),
            ..Default::default()
        };

        assert_eq!(
            singularity_env_vars(&singularity_config, &fixed_env_vars, &step_env_vars),
            vec![
                (
                    "SINGULARITYENV_AGENTFLOW_WORKDIR".to_string(),
                    "/tmp/af-step-work".to_string()
                ),
                (
                    "SINGULARITYENV_AGENTFLOW_INPUTS_JSON".to_string(),
                    "/tmp/af-step-work/inputs.json".to_string()
                ),
                (
                    "SINGULARITYENV_AGENTFLOW_PARAMS_JSON".to_string(),
                    "/tmp/af-step-work/params.json".to_string()
                ),
                (
                    "SINGULARITYENV_AGENTFLOW_OUTPUTS_JSON".to_string(),
                    "/tmp/af-step-work/outputs.json".to_string()
                ),
                (
                    "SINGULARITYENV_AGENTFLOW_INPUT_READS".to_string(),
                    "/tmp/af-step-work/inputs/reads/in.fastq".to_string()
                ),
                (
                    "SINGULARITYENV_AGENTFLOW_PARAM_MODE".to_string(),
                    "strict".to_string()
                ),
                (
                    "SINGULARITYENV_AGENTFLOW_OUTPUT_REPORT".to_string(),
                    "/tmp/af-step-work/outputs/report".to_string()
                ),
            ]
        );
        assert!(singularity_env_vars(&docker_config, &fixed_env_vars, &step_env_vars).is_empty());
        assert!(
            singularity_env_vars(&RunConfig::default(), &fixed_env_vars, &step_env_vars).is_empty()
        );
    }

    #[test]
    fn agentflow_env_names_collects_fixed_and_port_runtime_vars() {
        let step_env_vars = vec![
            (
                "AGENTFLOW_INPUT_READS".to_string(),
                "/tmp/af-step-work/inputs/reads/in.fastq".to_string(),
            ),
            ("AGENTFLOW_PARAM_MODE".to_string(), "strict".to_string()),
            (
                "AGENTFLOW_OUTPUT_REPORT".to_string(),
                "/tmp/af-step-work/outputs/report".to_string(),
            ),
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        ];

        assert_eq!(
            agentflow_env_names(&step_env_vars),
            vec![
                "AGENTFLOW_WORKDIR",
                "AGENTFLOW_INPUTS_JSON",
                "AGENTFLOW_PARAMS_JSON",
                "AGENTFLOW_OUTPUTS_JSON",
                "AGENTFLOW_INPUT_READS",
                "AGENTFLOW_PARAM_MODE",
                "AGENTFLOW_OUTPUT_REPORT",
            ]
        );
    }

    #[test]
    fn conda_micromamba_backends_preserve_argv_difference() {
        // conda keeps --no-capture-output; micromamba does not. (equivalence proof)
        let staged_inputs = BTreeMap::new();
        let ctx = test_exec_context(&staged_inputs);

        assert_eq!(
            prepare_runtime_command(&env_runtime("conda"), &ctx)
                .unwrap()
                .argv(),
            vec![
                "/opt/micromamba",
                "run",
                "--no-capture-output",
                "--name",
                "envA",
                "python",
                "tool.py",
            ]
        );
        assert_eq!(
            prepare_runtime_command(&env_runtime("micromamba"), &ctx)
                .unwrap()
                .argv(),
            vec![
                "/opt/micromamba",
                "run",
                "--name",
                "envA",
                "python",
                "tool.py"
            ]
        );
    }

    #[test]
    fn unknown_backend_keeps_unsupported_error() {
        let staged_inputs = BTreeMap::new();
        let ctx = test_exec_context(&staged_inputs);
        let err = prepare_runtime_command(&env_runtime("podman"), &ctx).unwrap_err();
        assert!(
            err.to_string()
                .contains("unsupported runtime.backend podman"),
            "unexpected error: {err}"
        );
        assert!(backend::backend_for("podman").is_none());
        assert!(backend::backend_for("local").is_some());
        assert!(backend::backend_for("conda").is_some());
        assert!(backend::backend_for("micromamba").is_some());
    }

    #[test]
    fn isolated_env_lockhash_is_content_and_platform_addressed() {
        let path = temp_project_path("isolated-lockhash");
        fs::create_dir_all(&path).unwrap();
        let env_file = path.join("environment.yml");
        fs::write(&env_file, "name: af-test\ndependencies:\n  - python=3.11\n").unwrap();

        let first = isolated_env_lock_hash_for_platform(&env_file, "linux-64").unwrap();
        let repeat = isolated_env_lock_hash_for_platform(&env_file, "linux-64").unwrap();
        assert_eq!(first, repeat);
        assert_eq!(
            isolated_env_prefix(&path, "marker/scan", &first),
            path.join(".agentflow")
                .join("envs")
                .join(format!("marker_scan@{first}"))
        );

        fs::write(&env_file, "name: af-test\ndependencies:\n  - python=3.12\n").unwrap();
        let changed_content = isolated_env_lock_hash_for_platform(&env_file, "linux-64").unwrap();
        assert_ne!(first, changed_content);

        fs::write(&env_file, "name: af-test\ndependencies:\n  - python=3.11\n").unwrap();
        let changed_platform = isolated_env_lock_hash_for_platform(&env_file, "osx-arm64").unwrap();
        assert_ne!(first, changed_platform);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn isolated_env_reuses_existing_lock_without_provisioning() {
        struct FakeProvisioner {
            calls: std::cell::Cell<usize>,
        }

        impl IsolatedEnvProvisioner for FakeProvisioner {
            fn ensure(
                &self,
                _env_file: &Path,
                prefix: &Path,
                _runner: &str,
            ) -> Result<(), StorageError> {
                self.calls.set(self.calls.get() + 1);
                fs::create_dir_all(prefix)?;
                fs::write(prefix.join(ISOLATED_ENV_LOCK_FILE), "fake explicit lock\n")?;
                Ok(())
            }
        }

        let path = temp_project_path("isolated-reuse");
        fs::create_dir_all(&path).unwrap();
        let env_file = path.join("environment.yml");
        fs::write(&env_file, "name: af-test\ndependencies:\n  - python=3.11\n").unwrap();
        let runtime = ToolRuntimeSpec {
            backend: "isolated-micromamba".to_string(),
            command: vec!["python".to_string(), "tool.py".to_string()],
            timeout_seconds: None,
            env_name: None,
            env_prefix: None,
            env_file: Some(env_file.display().to_string()),
            runner: Some("/opt/micromamba".to_string()),
            image: None,
        };
        let provisioner = FakeProvisioner {
            calls: std::cell::Cell::new(0),
        };

        let first = ensure_isolated_tool_environment_for_platform(
            &path,
            "marker/scan",
            &runtime,
            "linux-64",
            &provisioner,
        )
        .unwrap();
        assert_eq!(provisioner.calls.get(), 1);
        assert!(first.prefix.join(ISOLATED_ENV_LOCK_FILE).exists());

        let second = ensure_isolated_tool_environment_for_platform(
            &path,
            "marker/scan",
            &runtime,
            "linux-64",
            &provisioner,
        )
        .unwrap();
        assert_eq!(provisioner.calls.get(), 1);
        assert_eq!(first, second);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn isolated_micromamba_prepare_command_uses_managed_prefix() {
        let prefix = "/tmp/af-managed-prefix";
        let runtime = ToolRuntimeSpec {
            backend: "isolated-micromamba".to_string(),
            command: vec!["python".to_string(), "tool.py".to_string()],
            timeout_seconds: None,
            env_name: None,
            env_prefix: Some(prefix.to_string()),
            env_file: Some("environment.yml".to_string()),
            runner: Some("/opt/micromamba".to_string()),
            image: None,
        };
        let staged_inputs = BTreeMap::new();
        let ctx = test_exec_context(&staged_inputs);

        assert_eq!(
            prepare_runtime_command(&runtime, &ctx).unwrap().argv(),
            vec!["/opt/micromamba", "run", "-p", prefix, "python", "tool.py"]
        );
        assert!(backend::backend_for("isolated-micromamba").is_some());
    }

    #[test]
    fn runtime_config_json_adds_lockhash_only_for_isolated_backend() {
        let path = temp_project_path("isolated-runtime-config");
        fs::create_dir_all(&path).unwrap();
        let env_file = path.join("environment.yml");
        fs::write(&env_file, "name: af-test\ndependencies:\n  - python=3.11\n").unwrap();

        let conda = ToolRuntimeSpec {
            backend: "conda".to_string(),
            command: vec!["python".to_string(), "run.py".to_string()],
            timeout_seconds: None,
            env_name: Some("af-test".to_string()),
            env_prefix: None,
            env_file: None,
            runner: Some("/opt/conda/bin/conda".to_string()),
            image: None,
        };
        assert_eq!(
            runtime_config_json(&conda).unwrap(),
            "{\"backend\":\"conda\",\"command\":[\"python\",\"run.py\"],\"timeout_seconds\":null,\"env_name\":\"af-test\",\"env_prefix\":null,\"env_file\":null,\"env_file_hash\":null,\"runner\":\"/opt/conda/bin/conda\"}"
        );

        let isolated = ToolRuntimeSpec {
            backend: "isolated-micromamba".to_string(),
            command: vec!["python".to_string(), "run.py".to_string()],
            timeout_seconds: None,
            env_name: None,
            env_prefix: None,
            env_file: Some(env_file.display().to_string()),
            runner: Some("/opt/micromamba".to_string()),
            image: None,
        };
        let json =
            runtime_config_json_with_isolated_lock(&isolated, Some("fnv64:abc123".to_string()))
                .unwrap();
        assert!(json.contains("\"isolated_env_lock\":\"fnv64:abc123\""));
        let payload: RuntimeConfigJson = serde_json::from_str(&json).unwrap();
        assert_eq!(payload.isolated_env_lock.as_deref(), Some("fnv64:abc123"));

        let container = ToolRuntimeSpec {
            backend: "container".to_string(),
            command: vec!["python".to_string(), "run.py".to_string()],
            timeout_seconds: None,
            env_name: None,
            env_prefix: None,
            env_file: None,
            runner: Some("/usr/bin/docker".to_string()),
            image: Some("ghcr.io/acme/tool@sha256:0123456789abcdef".to_string()),
        };
        let container_json = runtime_config_json(&container).unwrap();
        assert_eq!(
            container_json,
            "{\"backend\":\"container\",\"command\":[\"python\",\"run.py\"],\"timeout_seconds\":null,\"env_name\":null,\"env_prefix\":null,\"env_file\":null,\"env_file_hash\":null,\"runner\":\"/usr/bin/docker\",\"container_image\":\"ghcr.io/acme/tool@sha256:0123456789abcdef\"}"
        );
        let payload: RuntimeConfigJson = serde_json::from_str(&container_json).unwrap();
        assert_eq!(
            payload.container_image.as_deref(),
            Some("ghcr.io/acme/tool@sha256:0123456789abcdef")
        );

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn status_json_is_exact_byte() {
        let path = temp_project_path("status-json");
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();
        let summary = store.summary().unwrap();
        let expected = format!(
            "{{\"schema_version\":\"agentflow.status.v0\",\"project\":{{\"id\":\"{}\",\"name\":\"Runtime Demo\",\"root_path\":\"{}\",\"engine_version\":\"{}\",\"created_at\":{},\"updated_at\":{}}},\"counts\":{{\"flows\":0,\"steps\":0,\"runs\":0,\"run_attempts\":0,\"artifacts\":0}}}}",
            summary.id,
            summary.root_path.display(),
            summary.engine_version,
            summary.created_at,
            summary.updated_at
        );

        assert_eq!(store.status_json().unwrap(), expected);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn sample_id_validator_rejects_duplicate_ids() {
        let path = temp_project_path("duplicate-sample-id");
        fs::create_dir_all(&path).unwrap();
        let table_path = path.join("expression.tsv");
        fs::write(&table_path, "sample\tTP53\nA\t1.2\nA\t0.4\n").unwrap();
        let port = crate::storage::ToolPortSpec {
            type_name: "TSV".to_string(),
            required: true,
            observer: None,
            profile: None,
            min_rows: None,
            required_columns: Vec::new(),
            sample_id_column: Some("sample".to_string()),
        };

        let err =
            sample_ids_for_input("expression_table", &table_path, &port, "sample").unwrap_err();
        assert!(err.to_string().contains("duplicate sample id A"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn sample_id_min_rows_counts_data_rows_not_header() {
        let path = temp_project_path("sample-id-min-rows");
        fs::create_dir_all(&path).unwrap();
        let table_path = path.join("expression.tsv");
        fs::write(&table_path, "sample\tTP53\n").unwrap();
        let port = crate::storage::ToolPortSpec {
            type_name: "TSV".to_string(),
            required: true,
            observer: None,
            profile: None,
            min_rows: Some(1),
            required_columns: Vec::new(),
            sample_id_column: Some("sample".to_string()),
        };

        let err = validate_port_file("input", "expression_table", &table_path, &port).unwrap_err();
        assert!(err.to_string().contains("has 0 rows"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn run_flow_executes_local_commands_and_resolves_step_outputs() {
        let path = temp_project_path("success");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();
        let script_path = write_script(&path);
        let command = script_path.display();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: marker_survival_scan
version: 0.1.0
maturity: wrapped
description: Scan a candidate marker against survival table
inputs:
  expression_table:
    type: TSV
    required: true
    required_columns: sample,TP53
    sample_id_column: sample
    min_rows: 1
  survival_table:
    type: TSV
    required: true
    required_columns: sample,time,status
    sample_id_column: sample
    min_rows: 1
params:
  gene:
    type: string
    required: true
outputs:
  marker_report:
    type: Markdown
    observer: marker_report
    min_rows: 3
runtime:
  backend: local
  command:
    - /bin/sh
    - {command}
"#
            ),
        );
        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: report
name: finalize_report
version: 0.1.0
maturity: wrapped
description: Finalize an upstream report
inputs:
  upstream_report:
    type: Markdown
    required: true
outputs:
  final_report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/sh
    - {command}
"#
            ),
        );

        let expression_path = path.join("expression.tsv");
        fs::write(&expression_path, "sample\tTP53\nA\t1.2\n").unwrap();
        let survival_path = path.join("survival.tsv");
        fs::write(&survival_path, "sample\ttime\tstatus\nA\t10\t1\n").unwrap();
        let expression_id = import_artifact(&store, expression_path);
        let survival_id = import_artifact(&store, survival_path);

        let flow = FlowDraft::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.flow.v0
id: marker_demo
name: Marker demo
steps:
  - id: artifact_scan
    tool: marker/marker_survival_scan
    reason: Evaluate TP53 marker signal
    needs: []
    inputs:
      expression_table: {expression_id}
      survival_table: {survival_id}
    params:
      gene: TP53
    outputs:
      marker_report: marker_report
  - id: finalize
    tool: report/finalize_report
    reason: Prepare final report artifact
    needs: [artifact_scan]
    inputs:
      upstream_report: artifact_scan.marker_report
    outputs:
      final_report: final_report
"#
        ))
        .unwrap();
        store.approve_flow(flow, None).unwrap();

        let pre_run_explanations = store.cache_explain_flow("marker_demo").unwrap();
        assert_eq!(pre_run_explanations.len(), 2);
        assert!(pre_run_explanations
            .iter()
            .all(|explanation| !explanation.hit));
        assert!(pre_run_explanations
            .iter()
            .any(|explanation| explanation.cache_key == "unavailable"));

        let summary = store.run_flow("marker_demo").unwrap();
        assert_eq!(summary.completed_steps, 2);
        assert_eq!(summary.failed_steps, 0);
        assert_eq!(summary.attempts.len(), 2);
        assert!(summary
            .attempts
            .iter()
            .all(|attempt| attempt.status == "succeeded"));

        let logs = store
            .read_logs(&summary.attempts.last().unwrap().run_id)
            .unwrap();
        assert!(logs.stdout.contains("finalize ok"));
        let runs = store.list_runs(Some("marker_demo")).unwrap();
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().all(|run| run.status == "completed"));
        let run_inspection = store
            .inspect_run_or_attempt(&summary.attempts[0].run_id)
            .unwrap();
        assert_eq!(run_inspection.run.flow_id, "marker_demo");
        assert_eq!(run_inspection.attempts.len(), 1);
        assert_eq!(run_inspection.attempts[0].status, "succeeded");
        let attempt_inspection = store
            .inspect_run_or_attempt(&summary.attempts[0].attempt_id)
            .unwrap();
        assert_eq!(attempt_inspection.run.run_id, summary.attempts[0].run_id);
        assert_eq!(
            store.inspect_flow("marker_demo").unwrap().steps[0].status,
            "completed"
        );
        assert_eq!(
            store.inspect_flow("marker_demo").unwrap().steps[1].status,
            "completed"
        );

        let computed = store
            .list_artifacts()
            .unwrap()
            .into_iter()
            .filter(|artifact| artifact.kind == "computed")
            .collect::<Vec<_>>();
        assert_eq!(computed.len(), 2);
        let observations = store.list_observations().unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].kind, "marker_report");
        assert!(observations[0].summary.contains("describes gene TP53"));
        assert_eq!(observations[0].metric_value("score"), Some(0.61));
        assert!(store
            .generate_report_markdown("marker_demo")
            .unwrap()
            .contains("kind `marker_report`"));
        assert!(store.status_json().unwrap().contains("\"run_attempts\":2"));

        let cached_flow = FlowDraft::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.flow.v0
id: marker_demo_cached
name: Marker demo cached
steps:
  - id: artifact_scan
    tool: marker/marker_survival_scan
    reason: Evaluate TP53 marker signal again
    needs: []
    inputs:
      expression_table: {expression_id}
      survival_table: {survival_id}
    params:
      gene: TP53
    outputs:
      marker_report: marker_report
  - id: finalize
    tool: report/finalize_report
    reason: Prepare final report artifact from cache
    needs: [artifact_scan]
    inputs:
      upstream_report: artifact_scan.marker_report
    outputs:
      final_report: final_report
"#
        ))
        .unwrap();
        store.approve_flow(cached_flow, None).unwrap();
        let cached_summary = store.run_flow("marker_demo_cached").unwrap();
        assert_eq!(cached_summary.completed_steps, 2);
        assert_eq!(cached_summary.failed_steps, 0);
        assert_eq!(
            cached_summary
                .attempts
                .iter()
                .map(|attempt| attempt.status.as_str())
                .collect::<Vec<_>>(),
            ["cache_hit", "cache_hit"]
        );
        let explanations = store.cache_explain_flow("marker_demo_cached").unwrap();
        assert_eq!(explanations.len(), 2);
        assert!(explanations.iter().all(|explanation| explanation.hit));
        let step_explanation = store
            .cache_explain_step_ref("marker_demo_cached.finalize")
            .unwrap();
        assert_eq!(step_explanation.len(), 1);
        assert_eq!(
            step_explanation[0].step_id,
            "step:marker_demo_cached/finalize"
        );
        assert!(step_explanation[0].hit);
        let target_explanation = store
            .cache_explain_target("marker_demo_cached/finalize")
            .unwrap();
        assert_eq!(target_explanation.len(), 1);
        assert!(target_explanation[0].hit);

        let computed_after_cache = store
            .list_artifacts()
            .unwrap()
            .into_iter()
            .filter(|artifact| artifact.kind == "computed")
            .count();
        assert_eq!(computed_after_cache, 4);
        let observations_after_cache = store.list_observations().unwrap();
        assert_eq!(observations_after_cache.len(), 2);
        assert!(observations_after_cache.iter().any(|observation| {
            observation.flow_id.as_deref() == Some("marker_demo_cached")
                && observation.kind == "marker_report"
        }));

        let cache_entries = store.list_cache_entries().unwrap();
        assert_eq!(cache_entries.len(), 2);
        assert!(cache_entries.iter().any(
            |entry| entry.tool_ref == "marker/marker_survival_scan" && entry.output_count == 1
        ));
        assert_eq!(
            store
                .prune_cache_entries(Some(31_536_000))
                .unwrap()
                .removed_entries,
            0
        );
        assert_eq!(
            store
                .prune_cache_entries(Some(u64::MAX))
                .unwrap()
                .removed_entries,
            0
        );
        assert_eq!(store.prune_cache_entries(None).unwrap().removed_entries, 2);
        assert!(store.list_cache_entries().unwrap().is_empty());

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn run_flow_stages_only_declared_inputs_inside_workdir() {
        let path = temp_project_path("input-staging");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();
        let script_path = path.join("record_input_path.sh");
        fs::write(
            &script_path,
            r#"printf 'declared=%s\n' "$AGENTFLOW_INPUT_DECLARED_TABLE"
printf 'inputs_json=%s\n' "$AGENTFLOW_INPUTS_JSON"
cat "$AGENTFLOW_INPUTS_JSON"
printf '\n'
cat "$AGENTFLOW_INPUT_DECLARED_TABLE" >/dev/null
printf 'ok\n' > "$AGENTFLOW_OUTPUT_REPORT"
"#,
        )
        .unwrap();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: staged_input_probe
version: 0.1.0
maturity: wrapped
description: Record the declared input path exposed to a local tool
inputs:
  declared_table:
    type: TSV
    required: true
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/sh
    - {}
"#,
                script_path.display()
            ),
        );

        let declared_source_path = path.join("declared.tsv");
        fs::write(&declared_source_path, "sample\tvalue\nA\t1\n").unwrap();
        let hidden_source_path = path.join("hidden.tsv");
        fs::write(&hidden_source_path, "sample\tvalue\nB\t2\n").unwrap();
        let declared_id = import_artifact(&store, declared_source_path);
        let _hidden_id = import_artifact(&store, hidden_source_path);
        let declared_store_path = store.inspect_artifact(&declared_id).unwrap().summary.path;

        let flow = FlowDraft::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.flow.v0
id: input_staging_demo
name: Input staging demo
steps:
  - id: probe
    tool: marker/staged_input_probe
    reason: Prove tools see staged declared inputs
    needs: []
    inputs:
      declared_table: {declared_id}
    outputs:
      report: marker_report
"#
        ))
        .unwrap();
        store.approve_flow(flow, None).unwrap();

        let summary = store.run_flow("input_staging_demo").unwrap();
        assert_eq!(summary.completed_steps, 1);
        assert_eq!(summary.failed_steps, 0);
        let workdir = &summary.attempts[0].workdir;
        let inputs_root = workdir.join("inputs");
        let mut input_ports = fs::read_dir(&inputs_root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        input_ports.sort();
        assert_eq!(input_ports, vec!["declared_table"]);
        assert!(!inputs_root.join("hidden_table").exists());

        let logs = store.read_logs(&summary.attempts[0].attempt_id).unwrap();
        let exposed_input_path = logs
            .stdout
            .lines()
            .find_map(|line| line.strip_prefix("declared="))
            .map(PathBuf::from)
            .expect("tool printed declared input path");
        let expected_staged_path = inputs_root.join("declared_table").join("declared.tsv");
        assert_eq!(exposed_input_path, expected_staged_path);
        assert!(exposed_input_path.starts_with(workdir));
        assert_ne!(exposed_input_path, declared_store_path);

        let inputs_json = fs::read_to_string(workdir.join("inputs.json")).unwrap();
        let input_map = parse_json_map(&inputs_json).unwrap();
        assert_eq!(
            PathBuf::from(input_map.get("declared_table").unwrap()),
            expected_staged_path
        );

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn stage_input_path_copies_when_symlink_fails() {
        let path = temp_project_path("input-staging-copy-fallback");
        fs::create_dir_all(&path).unwrap();
        let source_path = path.join("source.tsv");
        fs::write(&source_path, "sample\tvalue\nA\t1\n").unwrap();
        let inputs_root = path.join("work").join("inputs");

        let staged_path =
            stage_input_path_with_linker(&source_path, &inputs_root, "source_table", |_, _| {
                Err(std::io::Error::from_raw_os_error(18))
            })
            .unwrap();

        assert_eq!(
            staged_path,
            inputs_root.join("source_table").join("source.tsv")
        );
        assert_eq!(
            fs::read(&staged_path).unwrap(),
            fs::read(&source_path).unwrap()
        );
        #[cfg(unix)]
        assert!(!fs::symlink_metadata(&staged_path)
            .unwrap()
            .file_type()
            .is_symlink());

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn stage_input_path_force_copy_produces_real_file_not_symlink() {
        // Container backends mount only the workdir; a symlink into the artifact
        // store would dangle inside the container. force_copy must stage a real
        // file copy so it is readable through the workdir mount.
        let path = temp_project_path("input-staging-force-copy");
        fs::create_dir_all(&path).unwrap();
        let source_path = path.join("source.tsv");
        fs::write(&source_path, "sample\tvalue\nA\t1\n").unwrap();
        let inputs_root = path.join("work").join("inputs");

        let staged_path =
            stage_input_path(&source_path, &inputs_root, "source_table", true).unwrap();

        assert_eq!(
            fs::read(&staged_path).unwrap(),
            fs::read(&source_path).unwrap()
        );
        #[cfg(unix)]
        assert!(
            !fs::symlink_metadata(&staged_path)
                .unwrap()
                .file_type()
                .is_symlink(),
            "container staging must be a real copy, not a symlink"
        );

        // Without force_copy the default symlink boundary is used (unix).
        #[cfg(unix)]
        {
            let staged_symlink =
                stage_input_path(&source_path, &inputs_root, "linked_table", false).unwrap();
            assert!(fs::symlink_metadata(&staged_symlink)
                .unwrap()
                .file_type()
                .is_symlink());
        }

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn environment_backend_wraps_command_and_records_runtime_config() {
        let path = temp_project_path("micromamba-runtime");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();
        let script_path = write_script(&path);
        let runner_path = write_fake_environment_runner(&path);
        let command = script_path.display();
        let runner = runner_path.display();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: marker_survival_scan
version: 0.1.0
maturity: wrapped
description: Scan a candidate marker through an environment backend
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
  marker_report:
    type: Markdown
runtime:
  backend: micromamba
  runner: {runner}
  env_name: af-test
  command:
    - /bin/sh
    - {command}
"#
            ),
        );

        let expression_path = path.join("expression.tsv");
        fs::write(&expression_path, "sample\tTP53\nA\t1.2\n").unwrap();
        let survival_path = path.join("survival.tsv");
        fs::write(&survival_path, "sample\ttime\tstatus\nA\t10\t1\n").unwrap();
        let expression_id = import_artifact(&store, expression_path);
        let survival_id = import_artifact(&store, survival_path);
        let flow = FlowDraft::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.flow.v0
id: marker_demo
name: Marker demo
steps:
  - id: scan
    tool: marker/marker_survival_scan
    reason: Evaluate TP53 marker signal
    needs: []
    inputs:
      expression_table: {expression_id}
      survival_table: {survival_id}
    params:
      gene: TP53
    outputs:
      marker_report: marker_report
"#
        ))
        .unwrap();
        store.approve_flow(flow, None).unwrap();

        let summary = store.run_flow("marker_demo").unwrap();
        assert_eq!(summary.completed_steps, 1);
        assert_eq!(summary.failed_steps, 0);
        let logs = store.read_logs(&summary.attempts[0].attempt_id).unwrap();
        assert!(logs.stdout.contains("scan ok"));
        let command_text = fs::read_to_string(summary.attempts[0].workdir.join("command.sh"))
            .expect("command script");
        assert!(command_text.contains("fake_micromamba.sh"));
        assert!(command_text.contains("--name af-test"));
        let runtime_text = fs::read_to_string(summary.attempts[0].workdir.join("runtime.json"))
            .expect("runtime json");
        assert!(runtime_text.contains("\"backend\":\"micromamba\""));
        assert!(runtime_text.contains("\"env_name\":\"af-test\""));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn env_check_reports_local_backend_without_probe() {
        let path = temp_project_path("local-env-check");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();
        let script_path = write_script(&path);
        let command = script_path.display();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: local_tool
version: 0.1.0
maturity: wrapped
description: Local tool
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/sh
    - {command}
"#
            ),
        );

        let check = store.check_tool_environment("marker/local_tool").unwrap();
        assert!(check.ok);
        assert_eq!(check.backend, "local");
        assert_eq!(check.items[0].name, "backend");

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn env_check_runs_environment_probe_and_hashes_env_file() {
        let path = temp_project_path("env-check-probe");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();
        let runner_path = write_fake_environment_runner(&path);
        let env_file = path.join("environment.yml");
        fs::write(&env_file, "name: af-test\ndependencies:\n  - python=3.11\n").unwrap();
        let runner = runner_path.display();
        let env_file = env_file.display();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: env_tool
version: 0.1.0
maturity: wrapped
description: Environment tool
outputs:
  report:
    type: Markdown
runtime:
  backend: micromamba
  runner: {runner}
  env_name: af-test
  env_file: {env_file}
  command:
    - python
    - run.py
"#
            ),
        );

        let check = store.check_tool_environment("marker/env_tool").unwrap();
        assert!(check.ok);
        assert!(check.items.iter().any(|item| item.name == "env_file"
            && item
                .details
                .as_deref()
                .unwrap_or_default()
                .starts_with("fnv64:")));
        assert!(check
            .items
            .iter()
            .any(|item| item.name == "probe" && item.status == "ok"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn env_check_reports_missing_runner_without_probe() {
        let path = temp_project_path("env-check-missing-runner");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();

        register_tool(
            &store,
            r#"
schema_version: agentflow.tool.v0
namespace: marker
name: broken_env_tool
version: 0.1.0
maturity: wrapped
description: Environment tool with missing runner
outputs:
  report:
    type: Markdown
runtime:
  backend: micromamba
  runner: /not/a/runner
  env_name: af-test
  command:
    - python
    - run.py
"#
            .to_string(),
        );

        let check = store
            .check_tool_environment("marker/broken_env_tool")
            .unwrap();
        assert!(!check.ok);
        assert!(check
            .items
            .iter()
            .any(|item| item.name == "runner" && item.status == "failed"));
        assert!(check
            .items
            .iter()
            .any(|item| item.name == "probe" && item.status == "skipped"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn env_prepare_runs_explicit_environment_update() {
        let path = temp_project_path("env-prepare");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();
        let runner_path = write_fake_environment_runner(&path);
        let env_file = path.join("environment.yml");
        fs::write(&env_file, "name: af-test\ndependencies:\n  - python=3.11\n").unwrap();
        let runner = runner_path.display();
        let env_file = env_file.display();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: env_tool
version: 0.1.0
maturity: wrapped
description: Environment tool
outputs:
  report:
    type: Markdown
runtime:
  backend: micromamba
  runner: {runner}
  env_name: af-test
  env_file: {env_file}
  command:
    - python
    - run.py
"#
            ),
        );

        let summary = store.prepare_tool_environment("marker/env_tool").unwrap();
        assert!(summary.ok);
        assert_eq!(summary.status, "succeeded");
        assert_eq!(summary.exit_code, Some(0));
        assert!(summary.command.contains(&"env".to_string()));
        assert!(summary.command.contains(&"update".to_string()));
        assert!(summary.command.contains(&"--file".to_string()));
        assert!(summary.stdout.contains("fake env update"));
        assert!(summary
            .items
            .iter()
            .any(|item| item.name == "prepare" && item.status == "ok"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn env_prepare_requires_env_file_and_rejects_local_backend() {
        let path = temp_project_path("env-prepare-requires-env-file");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();
        let runner_path = write_fake_environment_runner(&path);
        let runner = runner_path.display();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: missing_env_file
version: 0.1.0
maturity: wrapped
description: Environment tool
outputs:
  report:
    type: Markdown
runtime:
  backend: micromamba
  runner: {runner}
  env_name: af-test
  command:
    - python
    - run.py
"#
            ),
        );
        let missing = store
            .prepare_tool_environment("marker/missing_env_file")
            .unwrap();
        assert!(!missing.ok);
        assert!(missing.command.is_empty());
        assert!(missing
            .items
            .iter()
            .any(|item| item.name == "env_file" && item.status == "failed"));

        register_tool(
            &store,
            r#"
schema_version: agentflow.tool.v0
namespace: marker
name: local_tool
version: 0.1.0
maturity: wrapped
description: Local tool
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#
            .to_string(),
        );
        let local = store.prepare_tool_environment("marker/local_tool").unwrap();
        assert!(!local.ok);
        assert!(local
            .items
            .iter()
            .any(|item| item.name == "backend" && item.status == "failed"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn conda_dependency_parser_extracts_top_level_packages_only() {
        let packages = extract_conda_dependency_packages(
            r#"
name: af-test
channels:
  - conda-forge
dependencies:
  - python=3.11
  - conda-forge::pandas>=2
  - pip:
    - scanpy==1.10
prefix: /tmp/af-test
"#,
        );

        assert_eq!(packages, vec!["pandas".to_string(), "python".to_string()]);
    }

    #[test]
    fn env_export_runs_environment_export_and_diffs_declared_packages() {
        let path = temp_project_path("env-export");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();
        let runner_path = write_fake_environment_runner(&path);
        let env_file = path.join("environment.yml");
        fs::write(
            &env_file,
            "name: af-test\ndependencies:\n  - python=3.11\n  - pandas\n  - numpy\n",
        )
        .unwrap();
        let runner = runner_path.display();
        let env_file = env_file.display();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: env_tool
version: 0.1.0
maturity: wrapped
description: Environment tool
outputs:
  report:
    type: Markdown
runtime:
  backend: micromamba
  runner: {runner}
  env_name: af-test
  env_file: {env_file}
  command:
    - python
    - run.py
"#
            ),
        );

        let summary = store.export_tool_environment("marker/env_tool").unwrap();
        assert!(!summary.ok);
        assert_eq!(summary.status, "succeeded");
        assert_eq!(summary.exit_code, Some(0));
        assert!(summary.command.contains(&"env".to_string()));
        assert!(summary.command.contains(&"export".to_string()));
        assert!(summary.export_hash.unwrap().starts_with("fnv64:"));
        assert_eq!(
            summary.declared_packages,
            vec![
                "numpy".to_string(),
                "pandas".to_string(),
                "python".to_string()
            ]
        );
        assert_eq!(
            summary.exported_packages,
            vec![
                "pandas".to_string(),
                "python".to_string(),
                "scanpy".to_string()
            ]
        );
        assert_eq!(summary.missing_packages, vec!["numpy".to_string()]);
        assert_eq!(summary.extra_packages, vec!["scanpy".to_string()]);
        assert!(summary
            .items
            .iter()
            .any(|item| item.name == "package_diff" && item.status == "failed"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn env_export_rejects_local_backend() {
        let path = temp_project_path("env-export-local");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();

        register_tool(
            &store,
            r#"
schema_version: agentflow.tool.v0
namespace: marker
name: local_tool
version: 0.1.0
maturity: wrapped
description: Local tool
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/echo
"#
            .to_string(),
        );

        let local = store.export_tool_environment("marker/local_tool").unwrap();
        assert!(!local.ok);
        assert!(local.command.is_empty());
        assert!(local.export_hash.is_none());
        assert!(local
            .items
            .iter()
            .any(|item| item.name == "backend" && item.status == "failed"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn run_flow_uses_absolute_output_env_for_relative_project_paths() {
        let path = PathBuf::from(format!(
            "target/agentflow-runtime-relative-{}-{}",
            std::process::id(),
            now_unix_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Relative Demo")).unwrap();
        let script_path = path.join("absolute_output_check.sh");
        fs::write(
            &script_path,
            r#"#!/bin/sh
case "$AGENTFLOW_OUTPUT_REPORT" in
  /*) ;;
  *) echo "output path is not absolute: $AGENTFLOW_OUTPUT_REPORT" >&2; exit 7 ;;
esac
if [ ! -d "$(dirname "$AGENTFLOW_OUTPUT_REPORT")" ]; then
  echo "output parent missing: $AGENTFLOW_OUTPUT_REPORT" >&2
  exit 8
fi
printf 'absolute output ok\n' > "$AGENTFLOW_OUTPUT_REPORT"
"#,
        )
        .unwrap();
        let script_path = fs::canonicalize(&script_path).unwrap();
        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: report
name: absolute_output_check
version: 0.1.0
maturity: wrapped
description: Verify output env paths are absolute
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/sh
    - {}
"#,
                script_path.display()
            ),
        );
        let flow = FlowDraft::from_simple_yaml(
            r#"
schema_version: agentflow.flow.v0
id: absolute_output_demo
name: Absolute output demo
steps:
  - id: check
    tool: report/absolute_output_check
    reason: Ensure output env path is absolute
    needs: []
    outputs:
      report: report
"#,
        )
        .unwrap();
        store.approve_flow(flow, None).unwrap();

        let summary = store.run_flow("absolute_output_demo").unwrap();
        assert_eq!(summary.completed_steps, 1);
        assert_eq!(summary.failed_steps, 0);
        assert!(summary.attempts[0].workdir.is_absolute());

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn runtime_config_hash_includes_environment_file_contents() {
        let path = temp_project_path("runtime-env-file-hash");
        fs::create_dir_all(&path).unwrap();
        let env_file = path.join("environment.yml");
        fs::write(&env_file, "name: af-test\ndependencies:\n  - python=3.11\n").unwrap();

        let runtime = ToolRuntimeSpec {
            backend: "conda".to_string(),
            command: vec!["python".to_string(), "run.py".to_string()],
            timeout_seconds: None,
            env_name: Some("af-test".to_string()),
            env_prefix: None,
            env_file: Some(env_file.display().to_string()),
            runner: Some("/opt/conda/bin/conda".to_string()),
            image: None,
        };
        let first = runtime_config_json(&runtime).unwrap();
        fs::write(&env_file, "name: af-test\ndependencies:\n  - python=3.12\n").unwrap();
        let second = runtime_config_json(&runtime).unwrap();
        assert_ne!(first, second);
        assert!(first.contains("\"env_file_hash\":\"fnv64:"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn run_step_ref_executes_only_selected_ready_step() {
        let path = temp_project_path("run-step");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();
        let script_path = write_script(&path);
        let command = script_path.display();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: marker_survival_scan
version: 0.1.0
maturity: wrapped
description: Scan a candidate marker against survival table
inputs:
  expression_table:
    type: TSV
    required: true
    required_columns: sample,TP53
    sample_id_column: sample
    min_rows: 1
  survival_table:
    type: TSV
    required: true
    required_columns: sample,time,status
    sample_id_column: sample
    min_rows: 1
params:
  gene:
    type: string
    required: true
outputs:
  marker_report:
    type: Markdown
    observer: marker_report
    min_rows: 3
runtime:
  backend: local
  command:
    - /bin/sh
    - {command}
"#
            ),
        );
        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: report
name: finalize_report
version: 0.1.0
maturity: wrapped
description: Finalize an upstream report
inputs:
  upstream_report:
    type: Markdown
    required: true
outputs:
  final_report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/sh
    - {command}
"#
            ),
        );

        let expression_path = path.join("expression.tsv");
        fs::write(&expression_path, "sample\tTP53\nA\t1.2\n").unwrap();
        let survival_path = path.join("survival.tsv");
        fs::write(&survival_path, "sample\ttime\tstatus\nA\t10\t1\n").unwrap();
        let expression_id = import_artifact(&store, expression_path);
        let survival_id = import_artifact(&store, survival_path);
        let flow = FlowDraft::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.flow.v0
id: marker_demo
name: Marker demo
steps:
  - id: artifact_scan
    tool: marker/marker_survival_scan
    reason: Evaluate TP53 marker signal
    needs: []
    inputs:
      expression_table: {expression_id}
      survival_table: {survival_id}
    params:
      gene: TP53
    outputs:
      marker_report: marker_report
  - id: finalize
    tool: report/finalize_report
    reason: Prepare final report artifact
    needs: [artifact_scan]
    inputs:
      upstream_report: artifact_scan.marker_report
    outputs:
      final_report: final_report
"#
        ))
        .unwrap();
        store.approve_flow(flow, None).unwrap();

        let blocked = store.run_step_ref("marker_demo.finalize").unwrap_err();
        assert!(blocked
            .to_string()
            .contains("before dependencies complete: artifact_scan"));

        let first = store.run_step_ref("marker_demo.artifact_scan").unwrap();
        assert_eq!(first.completed_steps, 1);
        assert_eq!(first.failed_steps, 0);
        assert_eq!(first.attempts.len(), 1);
        assert_eq!(first.attempts[0].status, "succeeded");
        let flow = store.inspect_flow("marker_demo").unwrap();
        assert_eq!(flow.steps[0].status, "completed");
        assert_eq!(flow.steps[1].status, "draft");

        let second = store.run_step_ref("step:marker_demo/finalize").unwrap();
        assert_eq!(second.completed_steps, 1);
        assert_eq!(second.failed_steps, 0);
        assert_eq!(second.attempts[0].status, "succeeded");

        let rerun = store.run_step_ref("marker_demo.artifact_scan").unwrap_err();
        assert!(rerun
            .to_string()
            .contains("run-step supports draft, ready, or failed steps only"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn run_flow_enforces_declared_input_validators_before_command() {
        let path = temp_project_path("input-validator");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();
        let script_path = path.join("should_not_run.sh");
        fs::write(
            &script_path,
            r#"printf 'should not run\n' > "$AGENTFLOW_OUTPUT_REPORT"
echo "unexpected execution"
"#,
        )
        .unwrap();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: validated_scan
version: 0.1.0
maturity: wrapped
description: Require a missing column
inputs:
  expression_table:
    type: TSV
    required: true
    required_columns: sample,missing_gene
    min_rows: 1
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/sh
    - {}
"#,
                script_path.display()
            ),
        );

        let expression_path = path.join("expression.tsv");
        fs::write(&expression_path, "sample\tTP53\nA\t1.2\n").unwrap();
        let expression_id = import_artifact(&store, expression_path);
        let flow = FlowDraft::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.flow.v0
id: validator_demo
name: Validator demo
steps:
  - id: scan
    tool: marker/validated_scan
    reason: Prove input validators run before command execution
    needs: []
    inputs:
      expression_table: {expression_id}
    outputs:
      report: marker_report
"#
        ))
        .unwrap();
        store.approve_flow(flow, None).unwrap();

        let summary = store.run_flow("validator_demo").unwrap();
        assert_eq!(summary.completed_steps, 0);
        assert_eq!(summary.failed_steps, 1);
        assert_eq!(summary.attempts[0].status, "failed");

        let logs = store.read_logs(&summary.attempts[0].attempt_id).unwrap();
        assert!(!logs.stdout.contains("unexpected execution"));
        assert!(logs
            .stderr
            .contains("input expression_table missing required column missing_gene"));
        let computed = store
            .list_artifacts()
            .unwrap()
            .into_iter()
            .filter(|artifact| artifact.kind == "computed")
            .count();
        assert_eq!(computed, 0);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn run_flow_enforces_sample_id_consistency_before_command() {
        let path = temp_project_path("sample-id-validator");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();
        let script_path = path.join("should_not_run_sample.sh");
        fs::write(
            &script_path,
            r#"printf 'should not run\n' > "$AGENTFLOW_OUTPUT_REPORT"
echo "unexpected sample execution"
"#,
        )
        .unwrap();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: sample_checked_scan
version: 0.1.0
maturity: wrapped
description: Require matching sample ids
inputs:
  expression_table:
    type: TSV
    required: true
    required_columns: sample,TP53
    sample_id_column: sample
  survival_table:
    type: TSV
    required: true
    required_columns: sample,time,status
    sample_id_column: sample
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/sh
    - {}
"#,
                script_path.display()
            ),
        );

        let expression_path = path.join("expression.tsv");
        fs::write(&expression_path, "sample\tTP53\nA\t1.2\nB\t0.4\n").unwrap();
        let survival_path = path.join("survival.tsv");
        fs::write(&survival_path, "sample\ttime\tstatus\nA\t10\t1\nC\t8\t0\n").unwrap();
        let expression_id = import_artifact(&store, expression_path);
        let survival_id = import_artifact(&store, survival_path);
        let flow = FlowDraft::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.flow.v0
id: sample_validator_demo
name: Sample validator demo
steps:
  - id: scan
    tool: marker/sample_checked_scan
    reason: Prove sample ids match before command execution
    needs: []
    inputs:
      expression_table: {expression_id}
      survival_table: {survival_id}
    outputs:
      report: marker_report
"#
        ))
        .unwrap();
        store.approve_flow(flow, None).unwrap();

        let summary = store.run_flow("sample_validator_demo").unwrap();
        assert_eq!(summary.completed_steps, 0);
        assert_eq!(summary.failed_steps, 1);
        assert_eq!(summary.attempts[0].status, "failed");

        let logs = store.read_logs(&summary.attempts[0].attempt_id).unwrap();
        assert!(!logs.stdout.contains("unexpected sample execution"));
        assert!(logs.stderr.contains("sample ids differ"));
        assert!(logs.stderr.contains("missing [B]"));
        assert!(logs.stderr.contains("extra [C]"));
        let computed = store
            .list_artifacts()
            .unwrap()
            .into_iter()
            .filter(|artifact| artifact.kind == "computed")
            .count();
        assert_eq!(computed, 0);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn run_flow_enforces_declared_output_validators_before_publish() {
        let path = temp_project_path("output-validator");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();
        let script_path = path.join("short_report.sh");
        fs::write(
            &script_path,
            r#"cat "$AGENTFLOW_INPUT_EXPRESSION_TABLE" >/dev/null
printf 'too short\n' > "$AGENTFLOW_OUTPUT_REPORT"
echo "wrote short report"
"#,
        )
        .unwrap();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: short_report_scan
version: 0.1.0
maturity: wrapped
description: Write an output that fails min_rows
inputs:
  expression_table:
    type: TSV
    required: true
outputs:
  report:
    type: Markdown
    min_rows: 3
runtime:
  backend: local
  command:
    - /bin/sh
    - {}
"#,
                script_path.display()
            ),
        );

        let expression_path = path.join("expression.tsv");
        fs::write(&expression_path, "sample\tTP53\nA\t1.2\n").unwrap();
        let expression_id = import_artifact(&store, expression_path);
        let flow = FlowDraft::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.flow.v0
id: output_validator_demo
name: Output validator demo
steps:
  - id: scan
    tool: marker/short_report_scan
    reason: Prove output validators block bad artifacts
    needs: []
    inputs:
      expression_table: {expression_id}
    outputs:
      report: marker_report
"#
        ))
        .unwrap();
        store.approve_flow(flow, None).unwrap();

        let summary = store.run_flow("output_validator_demo").unwrap();
        assert_eq!(summary.completed_steps, 0);
        assert_eq!(summary.failed_steps, 1);
        assert_eq!(summary.attempts[0].status, "failed");

        let logs = store.read_logs(&summary.attempts[0].attempt_id).unwrap();
        assert!(logs.stdout.contains("wrote short report"));
        assert!(logs
            .stderr
            .contains("output report has 1 rows, expected at least 3"));
        let flow = store.inspect_flow("output_validator_demo").unwrap();
        assert_eq!(flow.steps[0].status, "failed");
        let computed = store
            .list_artifacts()
            .unwrap()
            .into_iter()
            .filter(|artifact| artifact.kind == "computed")
            .count();
        assert_eq!(computed, 0);

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn run_flow_marks_step_failed_and_preserves_logs() {
        let path = temp_project_path("failure");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();
        let script_path = path.join("fail_tool.sh");
        fs::write(
            &script_path,
            r#"echo "failure stdout"
echo "boom" >&2
exit 3
"#,
        )
        .unwrap();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: failing_scan
version: 0.1.0
maturity: wrapped
description: Fail deliberately
inputs:
  expression_table:
    type: TSV
    required: true
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/sh
    - {}
"#,
                script_path.display()
            ),
        );

        let expression_path = path.join("expression.tsv");
        fs::write(&expression_path, "sample\tTP53\nA\t1.2\n").unwrap();
        let expression_id = import_artifact(&store, expression_path);
        let flow = FlowDraft::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.flow.v0
id: failing_demo
name: Failing demo
steps:
  - id: scan
    tool: marker/failing_scan
    reason: Prove failed attempts retain logs
    needs: []
    inputs:
      expression_table: {expression_id}
    outputs:
      report: marker_report
"#
        ))
        .unwrap();
        store.approve_flow(flow, None).unwrap();

        let summary = store.run_flow("failing_demo").unwrap();
        assert_eq!(summary.completed_steps, 0);
        assert_eq!(summary.failed_steps, 1);
        assert_eq!(summary.attempts[0].status, "failed");
        assert_eq!(summary.attempts[0].exit_code, Some(3));

        let logs = store.read_logs(&summary.attempts[0].attempt_id).unwrap();
        assert!(logs.stdout.contains("failure stdout"));
        assert!(logs.stderr.contains("boom"));
        assert_eq!(
            store.inspect_flow("failing_demo").unwrap().steps[0].status,
            "failed"
        );

        let retry = store.retry_step_ref("failing_demo.scan").unwrap();
        assert_eq!(retry.completed_steps, 0);
        assert_eq!(retry.failed_steps, 1);
        assert_eq!(retry.attempts[0].status, "failed");
        assert!(store.status_json().unwrap().contains("\"run_attempts\":2"));

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn run_flow_marks_timed_out_attempt_and_does_not_publish_outputs() {
        let path = temp_project_path("timeout");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();

        register_tool(
            &store,
            r#"
schema_version: agentflow.tool.v0
namespace: marker
name: sleepy_scan
version: 0.1.0
maturity: wrapped
description: Sleep longer than the configured timeout
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  timeout_seconds: 1
  command:
    - /bin/sleep
    - 2
"#
            .to_string(),
        );

        let flow = FlowDraft::from_simple_yaml(
            r#"
schema_version: agentflow.flow.v0
id: timeout_demo
name: Timeout demo
steps:
  - id: scan
    tool: marker/sleepy_scan
    reason: Prove timeout attempts fail without publishing outputs
    needs: []
    outputs:
      report: marker_report
"#,
        )
        .unwrap();
        store.approve_flow(flow, None).unwrap();

        let summary = store.run_flow("timeout_demo").unwrap();
        assert_eq!(summary.completed_steps, 0);
        assert_eq!(summary.failed_steps, 1);
        assert_eq!(summary.attempts[0].status, "timed_out");
        assert_eq!(
            store.inspect_flow("timeout_demo").unwrap().steps[0].status,
            "failed"
        );

        let logs = store.read_logs(&summary.attempts[0].attempt_id).unwrap();
        assert!(logs.stderr.contains("command timed out after 1 seconds"));
        let computed = store
            .list_artifacts()
            .unwrap()
            .into_iter()
            .filter(|artifact| artifact.kind == "computed")
            .count();
        assert_eq!(computed, 0);
        assert!(store.list_cache_entries().unwrap().is_empty());

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn run_flow_rejects_outputs_that_escape_workdir_by_symlink() {
        let path = temp_project_path("escaped-output");
        fs::create_dir_all(&path).unwrap();
        let store = ProjectStore::init(&path, Some("Runtime Demo")).unwrap();
        let script_path = path.join("escape_tool.sh");
        fs::write(
            &script_path,
            r#"ln -sf "$AGENTFLOW_INPUT_EXPRESSION_TABLE" "$AGENTFLOW_OUTPUT_REPORT"
echo "wrote symlink"
"#,
        )
        .unwrap();

        register_tool(
            &store,
            format!(
                r#"
schema_version: agentflow.tool.v0
namespace: marker
name: escape_scan
version: 0.1.0
maturity: wrapped
description: Try to publish an escaped symlink
inputs:
  expression_table:
    type: TSV
    required: true
outputs:
  report:
    type: Markdown
runtime:
  backend: local
  command:
    - /bin/sh
    - {}
"#,
                script_path.display()
            ),
        );

        let expression_path = path.join("expression.tsv");
        fs::write(&expression_path, "sample\tTP53\nA\t1.2\n").unwrap();
        let expression_id = import_artifact(&store, expression_path);
        let flow = FlowDraft::from_simple_yaml(&format!(
            r#"
schema_version: agentflow.flow.v0
id: escape_demo
name: Escape demo
steps:
  - id: scan
    tool: marker/escape_scan
    reason: Prove escaped outputs are rejected
    needs: []
    inputs:
      expression_table: {expression_id}
    outputs:
      report: marker_report
"#
        ))
        .unwrap();
        store.approve_flow(flow, None).unwrap();

        let summary = store.run_flow("escape_demo").unwrap();
        assert_eq!(summary.completed_steps, 0);
        assert_eq!(summary.failed_steps, 1);
        assert_eq!(summary.attempts[0].status, "failed");
        let logs = store.read_logs(&summary.attempts[0].attempt_id).unwrap();
        assert!(logs.stdout.contains("wrote symlink"));
        let computed = store
            .list_artifacts()
            .unwrap()
            .into_iter()
            .filter(|artifact| artifact.kind == "computed")
            .count();
        assert_eq!(computed, 0);

        let _ = fs::remove_dir_all(path);
    }
}
