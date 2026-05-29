use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::{params, OptionalExtension};

use crate::domain::{RunAttemptStatus, StepStatus};
use crate::storage::{
    project_dir, ComputedArtifactRequest, ProjectStore, StorageError, StoredFlowStep,
    ToolRuntimeSpec,
};

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

    pub fn run_flow(&self, flow_id: &str) -> Result<FlowRunSummary, StorageError> {
        if self.inspect_flow(flow_id)?.status != "approved" {
            return Err(StorageError::InvalidInput(format!(
                "flow {flow_id} must be approved before run"
            )));
        }

        let mut completed_steps = 0;
        let mut failed_steps = 0;
        let mut attempts = Vec::new();

        loop {
            let flow = self.inspect_flow(flow_id)?;
            let mut completed = completed_step_ids(&flow.steps);
            let ready = ready_steps(&flow.steps, &flow.edges, &completed);
            if ready.is_empty() {
                break;
            }

            let mut progressed = false;
            for step in ready {
                let attempt = self.run_step(flow_id, &step)?;
                match attempt.status.as_str() {
                    "succeeded" | "cache_hit" => {
                        completed.insert(attempt.step_id.clone());
                        completed_steps += 1;
                        progressed = true;
                    }
                    _ => {
                        failed_steps += 1;
                    }
                }
                attempts.push(attempt);
                if failed_steps > 0 {
                    break;
                }
            }

            if failed_steps > 0 || !progressed {
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

        let attempt = self.run_step(&flow_id, step)?;
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
            stdout: fs::read_to_string(&stdout_path)?,
            stderr: fs::read_to_string(&stderr_path)?,
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

        Ok(format!(
            concat!(
                "{{",
                "\"schema_version\":\"{}\",",
                "\"project\":{{",
                "\"id\":\"{}\",",
                "\"name\":\"{}\",",
                "\"root_path\":\"{}\",",
                "\"engine_version\":\"{}\",",
                "\"created_at\":{},",
                "\"updated_at\":{}",
                "}},",
                "\"counts\":{{",
                "\"flows\":{},",
                "\"steps\":{},",
                "\"runs\":{},",
                "\"run_attempts\":{},",
                "\"artifacts\":{}",
                "}}",
                "}}"
            ),
            agentflow_schemas::STATUS_JSON_SCHEMA_V0,
            escape_json(&summary.id),
            escape_json(&summary.name),
            escape_json(&summary.root_path.display().to_string()),
            escape_json(&summary.engine_version),
            summary.created_at,
            summary.updated_at,
            flow_count,
            step_count,
            run_count,
            attempt_count,
            artifact_count
        ))
    }

    fn run_step(
        &self,
        flow_id: &str,
        step: &StoredFlowStep,
    ) -> Result<AttemptSummary, StorageError> {
        let tool_ref = step.tool_ref.as_deref().ok_or_else(|| {
            StorageError::InvalidInput(format!("step {} has no tool_ref", step.id))
        })?;
        let tool = self.executable_tool(tool_ref)?;
        let inputs = parse_json_map(&step.inputs_json)?;
        let params_map = parse_json_map(&step.params_json)?;
        let outputs = parse_json_map(&step.outputs_json)?;
        let resolved_inputs = self.resolve_inputs(flow_id, &inputs)?;
        let resolved_input_paths = input_paths(&resolved_inputs);
        let params_json = string_map_json(&params_map);
        let input_hashes_json = input_hashes_json(&resolved_inputs);
        let runtime_config = runtime_config_json(&tool.runtime)?;
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
        let stdout_path = workdir.join("stdout.log");
        let stderr_path = workdir.join("stderr.log");
        let resolved_outputs = output_paths(&workdir, &outputs);
        fs::create_dir_all(resolved_outputs.root())?;

        let prepared_command = prepare_runtime_command(&tool.runtime)?;
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
            return self.finish_attempt(FinishAttempt {
                run_id,
                attempt_id,
                step_id: step.id.clone(),
                workdir,
                stdout_path,
                stderr_path,
                status: RunAttemptStatus::Failed,
                exit_code: None,
                error_message: Some(error.to_string()),
            });
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
            return self.finish_attempt(FinishAttempt {
                run_id,
                attempt_id,
                step_id: step.id.clone(),
                workdir,
                stdout_path,
                stderr_path,
                status,
                exit_code: None,
                error_message,
            });
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
            .envs(env_vars(
                &resolved_input_paths,
                &params_map,
                resolved_outputs.as_map(),
            ));
        let output = run_local_command(command, tool.runtime.timeout_seconds);

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
                        Ok(()) => match validate_declared_outputs(&resolved_outputs, &tool.outputs)
                        {
                            Ok(()) => {
                                let mut published_outputs = BTreeMap::new();
                                let publish_result = resolved_outputs.as_map().iter().try_for_each(
                                    |(output_name, output_path)| {
                                        let artifact_type = tool
                                            .outputs
                                            .get(output_name)
                                            .map(|port| port.type_name.clone())
                                            .unwrap_or_else(|| "File".to_string());
                                        let artifact = self.register_computed_artifact(
                                            ComputedArtifactRequest {
                                                source_path: output_path.clone(),
                                                artifact_type,
                                                output_name: output_name.clone(),
                                                source_step_id: step.id.clone(),
                                                source_run_id: run_id.clone(),
                                            },
                                        )?;
                                        self.observe_declared_output(
                                            &artifact.summary.id,
                                            output_name,
                                            &tool.outputs,
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
            step_id: step.id.clone(),
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
        let runtime_hash = stable_hash(&runtime_config_json(&tool.runtime)?);
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
            if json_string_field(&validation_json, "output_name").as_deref() == Some(output_name) {
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

fn input_paths(inputs: &BTreeMap<String, ResolvedInput>) -> BTreeMap<String, PathBuf> {
    inputs
        .iter()
        .map(|(name, input)| (name.clone(), input.path.clone()))
        .collect()
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
    let env_file_hash = runtime
        .env_file
        .as_deref()
        .map(|path| file_hash_fnv64(Path::new(path)))
        .transpose()?;
    Ok(format!(
        concat!(
            "{{",
            "\"backend\":\"{}\",",
            "\"command\":{},",
            "\"timeout_seconds\":{},",
            "\"env_name\":{},",
            "\"env_prefix\":{},",
            "\"env_file\":{},",
            "\"env_file_hash\":{},",
            "\"runner\":{}",
            "}}"
        ),
        escape_json(&runtime.backend),
        string_array_json(&runtime.command),
        runtime
            .timeout_seconds
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".to_string()),
        optional_json_string(runtime.env_name.as_deref()),
        optional_json_string(runtime.env_prefix.as_deref()),
        optional_json_string(runtime.env_file.as_deref()),
        optional_json_string(env_file_hash.as_deref()),
        optional_json_string(runtime.runner.as_deref())
    ))
}

fn optional_json_string(value: Option<&str>) -> String {
    value
        .map(|value| format!("\"{}\"", escape_json(value)))
        .unwrap_or_else(|| "null".to_string())
}

fn stable_hash(input: &str) -> String {
    stable_hash_bytes(input.as_bytes())
}

fn file_hash_fnv64(path: &Path) -> Result<String, StorageError> {
    Ok(stable_hash_bytes(&fs::read(path)?))
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

fn prepare_runtime_command(
    runtime: &ToolRuntimeSpec,
) -> Result<PreparedRuntimeCommand, StorageError> {
    match runtime.backend.as_str() {
        "local" => {
            let executable = runtime.command.first().ok_or_else(|| {
                StorageError::InvalidInput("runtime.command must not be empty".to_string())
            })?;
            Ok(PreparedRuntimeCommand {
                executable: executable.clone(),
                args: runtime.command.iter().skip(1).cloned().collect(),
            })
        }
        "conda" | "micromamba" => {
            let runner = runtime.runner.as_ref().ok_or_else(|| {
                StorageError::InvalidInput(
                    "environment runtime must declare absolute runner path".to_string(),
                )
            })?;
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
        other => Err(StorageError::InvalidInput(format!(
            "unsupported runtime.backend {other}"
        ))),
    }
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
        let output = command.output()?;
        return Ok(LocalCommandOutput {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
            timed_out: false,
            timeout_seconds: None,
        });
    };

    let timeout = Duration::from_secs(timeout_seconds);
    let started = Instant::now();
    let mut child = command.spawn()?;
    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            return Ok(LocalCommandOutput {
                status: output.status,
                stdout: output.stdout,
                stderr: output.stderr,
                timed_out: false,
                timeout_seconds: Some(timeout_seconds),
            });
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let output = child.wait_with_output()?;
            return Ok(LocalCommandOutput {
                status: output.status,
                stdout: output.stdout,
                stderr: output.stderr,
                timed_out: true,
                timeout_seconds: Some(timeout_seconds),
            });
        }
        thread::sleep(Duration::from_millis(25));
    }
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
    let existing = fs::read_to_string(stderr_path).unwrap_or_default();
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

    let text = fs::read_to_string(path).map_err(|error| {
        StorageError::InvalidInput(format!(
            "{direction} {name} validator requires UTF-8 text at {}: {error}",
            path.display()
        ))
    })?;
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

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
    let text = fs::read_to_string(path).map_err(|error| {
        StorageError::InvalidInput(format!(
            "input {name} sample_id_column validator requires UTF-8 text at {}: {error}",
            path.display()
        ))
    })?;
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
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

fn parse_json_map(input: &str) -> Result<BTreeMap<String, String>, StorageError> {
    let mut map = BTreeMap::new();
    let mut index = 0;
    skip_json_whitespace(input, &mut index);
    expect_json_char(input, &mut index, '{')?;
    skip_json_whitespace(input, &mut index);
    if consume_json_char(input, &mut index, '}') {
        return Ok(map);
    }

    loop {
        let key = parse_json_string(input, &mut index)?;
        skip_json_whitespace(input, &mut index);
        expect_json_char(input, &mut index, ':')?;
        skip_json_whitespace(input, &mut index);
        let value = parse_json_string(input, &mut index)?;
        map.insert(key, value);
        skip_json_whitespace(input, &mut index);
        if consume_json_char(input, &mut index, ',') {
            skip_json_whitespace(input, &mut index);
            continue;
        }
        if consume_json_char(input, &mut index, '}') {
            break;
        }
        return Err(StorageError::InvalidInput(format!(
            "cannot parse map: {input}"
        )));
    }

    skip_json_whitespace(input, &mut index);
    if index != input.len() {
        return Err(StorageError::InvalidInput(format!(
            "cannot parse map: {input}"
        )));
    }
    Ok(map)
}

fn parse_json_string(input: &str, index: &mut usize) -> Result<String, StorageError> {
    expect_json_char(input, index, '"')?;
    let rest = input.get(*index..).ok_or_else(|| {
        StorageError::InvalidInput(format!("cannot parse json string in map: {input}"))
    })?;
    let end = find_json_string_end(rest)
        .ok_or_else(|| StorageError::InvalidInput(format!("cannot parse map: {input}")))?;
    let value = unescape_json_string(&rest[..end]);
    *index += end + 1;
    Ok(value)
}

fn expect_json_char(input: &str, index: &mut usize, expected: char) -> Result<(), StorageError> {
    if consume_json_char(input, index, expected) {
        Ok(())
    } else {
        Err(StorageError::InvalidInput(format!(
            "expected '{expected}' while parsing map: {input}"
        )))
    }
}

fn consume_json_char(input: &str, index: &mut usize, expected: char) -> bool {
    if input
        .get(*index..)
        .and_then(|rest| rest.chars().next())
        .is_some_and(|actual| actual == expected)
    {
        *index += expected.len_utf8();
        true
    } else {
        false
    }
}

fn skip_json_whitespace(input: &str, index: &mut usize) {
    while input
        .get(*index..)
        .and_then(|rest| rest.chars().next())
        .is_some_and(char::is_whitespace)
    {
        let ch = input[*index..].chars().next().expect("checked above");
        *index += ch.len_utf8();
    }
}

fn path_map_json(map: &BTreeMap<String, PathBuf>) -> String {
    let fields = map
        .iter()
        .map(|(key, value)| {
            format!(
                "\"{}\":\"{}\"",
                escape_json(key),
                escape_json(&value.display().to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{fields}}}")
}

fn string_map_json(map: &BTreeMap<String, String>) -> String {
    let fields = map
        .iter()
        .map(|(key, value)| format!("\"{}\":\"{}\"", escape_json(key), escape_json(value)))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{fields}}}")
}

fn string_array_json(values: &[String]) -> String {
    let items = values
        .iter()
        .map(|value| format!("\"{}\"", escape_json(value)))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
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

fn json_string_field(json: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\":\"");
    let start = json.find(&marker)? + marker.len();
    let rest = &json[start..];
    let end = find_json_string_end(rest)?;
    Some(unescape_json_string(&rest[..end]))
}

fn find_json_string_end(input: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(index),
            _ => {}
        }
    }
    None
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

fn now_unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
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
    use crate::storage::{
        ArtifactImportMode, ArtifactImportRequest, FlowDraft, ProjectStore, ToolSpec,
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
if [ "$1" != "run" ]; then
  echo "expected run subcommand" >&2
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

    #[test]
    fn runtime_json_map_parser_handles_punctuation_inside_strings() {
        let parsed =
            parse_json_map(r#"{"gene":"TP53,EGFR:ALK","label":"quoted \"value\""}"#).unwrap();
        assert_eq!(parsed["gene"], "TP53,EGFR:ALK");
        assert_eq!(parsed["label"], "quoted \"value\"");
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
