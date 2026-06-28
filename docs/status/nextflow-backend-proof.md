# Nextflow Backend Proof

## Motivation

The `nextflow` backend lets a Nextflow module behave like any other AgentFlow
tool: typed inputs and outputs are declared in the tool spec, the agent can
select it as a normal tool, and the scheduler can run it singly or as one step
in a chain without agent or scheduler changes.

## Design

For `runtime.backend: nextflow`, AgentFlow prepares:

```text
<runtime.runner> run <runtime.command[0]> <runtime.command[1..]>
```

`runtime.runner` is the absolute path to the Nextflow executable. The first
runtime command entry is the `.nf` module path, and later entries are passed as
extra Nextflow arguments such as `-profile standard`.

## Environment Convention

The backend reuses the existing local-tool convention. AgentFlow already
injects per-step values such as `AGENTFLOW_WORKDIR`, `AGENTFLOW_INPUT_*`,
`AGENTFLOW_PARAM_*`, `AGENTFLOW_OUTPUT_*`, and the JSON aggregate variables,
then starts the tool in the step workdir. The Nextflow module reads those
environment variables and writes declared outputs to the corresponding
`AGENTFLOW_OUTPUT_<NAME>` paths. No new I/O convention is introduced.

## Validation

`validate_runtime_backend` adds a `nextflow` arm: `runner` is required and must
be an absolute path; `env_name` / `env_prefix` / `env_file` / `image` are
rejected (meaningless for this backend); and `command[0]` (the `.nf` module)
must be absolute — it is resolved from the managed step workdir at run time, so
a relative path would silently fail. The shared `is_inline_interpreter_command`
and non-empty-argv checks still apply.

## Single vs composed invocation

Because a Nextflow module registers as an ordinary typed tool, the agent treats
it exactly like a local tool: run one module-tool on its own (single
invocation), or let the agent backward-chain several module-tools (and mix them
with local/container tools) by matching output types to input types (composed
invocation). No agent or scheduler changes were needed.

## Tests

- `backend.rs`: `nextflow_backend_builds_run_argv` (argv shape) and
  `nextflow_backend_missing_runner_is_error`.
- `tool_registry.rs`: `accepts_nextflow_runtime_with_absolute_runner_and_module`,
  `rejects_nextflow_runtime_with_relative_module_path`,
  `rejects_nextflow_runtime_without_runner`.
- Full `agentflow-core` suite green (369); clippy clean; `argument.rs` untouched.

## Caveats

- `prepare_step` runs with a cleared environment and `PATH=/usr/bin:/bin`, so
  `runtime.runner` may need to point at a wrapper that sets up Java, `HOME`, and
  `NXF_HOME` before invoking Nextflow.
- AgentFlow owns caching through `runtime_config`, which includes backend,
  command, and runner. Disable Nextflow's own `-resume` to avoid a second cache
  authority.
- Egress is the user's responsibility. Nextflow tools run with inherited env,
  not the container backend's `--network none`, which is consistent with forage
  scripts.
- The MVP is synchronous: `nextflow run` blocks until completion. Async and HPC
  job handles are future work.
