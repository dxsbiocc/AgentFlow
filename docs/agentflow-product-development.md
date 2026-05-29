# AgentFlow Product Development Document

Status: Draft for review
Owner: TBD
Last updated: 2026-05-28
MVP scope document: `docs/agentflow-v0-runtime-mvp-spec.md`

## 1. Product Thesis

AgentFlow is not merely a replacement for Nextflow, Snakemake, or notebooks.

AgentFlow is a product layer for **uncertainty-aware scientific analysis**: it helps researchers preserve the reasoning trail when an AI-assisted analysis changes direction, fails, branches, or produces negative evidence.

The core promise is:

> Make exploratory scientific analysis traceable enough that a researcher can understand why each step happened, what it observed, why it continued or stopped, and how new hypotheses emerged.

AgentFlow must own a unified task/runtime/state model because scientific analysis often starts from intermediate artifacts, not from the canonical pipeline beginning. A user may start from BAM, count matrix, H5AD, VCF, fusion table, differential expression result, or survival metadata. The product cannot assume every analysis begins at FASTQ.

V1 should be a runnable product, not only a planning document or graph viewer. It should implement the minimum runtime needed for AgentFlow's product promise: task graph, ready-step scheduling, runtime declaration, work directory, logs, status, retry, cache metadata, artifact registration, and validation.

The first implementation slice is intentionally narrower and is specified in `docs/agentflow-v0-runtime-mvp-spec.md`.

If AgentFlow becomes only a workflow executor, it is not worth building. If AgentFlow does not manage execution state at all, it will also fail, because provenance, validation, retry, resume, and branch decisions would be split across incompatible systems.

## 2. Problem Statement

Modern omics and biomedical analysis often starts with a goal, but the path changes while evidence accumulates.

Examples:

- A user wants to validate gene A as a tumor therapy marker, but analysis suggests gene A is weak while a homolog, interacting gene, upstream regulator, or pathway is more promising.
- A fusion gene detection branch ends because no confident fusion is detected; the negative result is meaningful, but traditional pipelines often treat it as an empty output.
- QC, batch effect, sample mismatch, missing annotation, tool failure, or weak signal forces the analysis to branch or stop.
- AI can suggest the next analysis, but the reasoning becomes scattered across chat messages, notebooks, logs, and temporary files.

The product problem is not simply "run tools."

The product problem is:

> AI-assisted research lacks a durable, reviewable causal record of goals, inputs, assumptions, observations, decisions, branches, failures, and revised hypotheses.

## 3. Product Positioning

AgentFlow should be the unified project runtime for AI-assisted scientific analysis.

| Layer | Product Role |
| --- | --- |
| Notebook | Flexible manual exploration |
| Nextflow/Snakemake | Optional backend/import/export target for stable deterministic pipelines |
| AgentFlow | Unified step execution, validation, observation, reasoning, branching, approval, and provenance |

Nextflow/Snakemake should be treated as optional execution backends or interoperability targets, not as the primary state model. AgentFlow needs its own step/run/artifact graph so it can start from arbitrary intermediate inputs and preserve one causal record.

The revised positioning is:

> AgentFlow owns the scientific state graph and task state. Existing workflow engines can be called from AgentFlow, imported into AgentFlow, or exported from AgentFlow, but they should not be the only execution substrate.

## 4. Target Users

### Primary User

Computational biologists and bioinformaticians who use AI to explore omics data and need a defensible record of why the analysis changed direction.

### Secondary User

Wet-lab or translational researchers who need to review an AI-assisted analysis path without reading every command, notebook cell, or log file.

### Tertiary User

Platform developers building AI-native scientific analysis products.

## 5. Jobs To Be Done

1. When I start an exploratory analysis, I want the system to create a clear draft plan, so that I can approve the intent before tools run.
2. When I already have intermediate artifacts, I want to start from BAM, count matrix, H5AD, VCF, or prior result tables, so that I do not need to rerun upstream steps just to enter the system.
3. When a step starts, I want to know why it is being run and what assumptions it depends on, so that the analysis has scientific context.
4. When a step receives input, I want the system to validate file existence, type, sample identity, schema, and parameter safety, so that failures are caught early.
5. When a step runs, I want its environment, command, logs, retry state, and resource status tracked in one place, so that I can manage execution without switching systems.
6. When a step finishes, I want the output validated and summarized, so that I can understand the evidence without opening every artifact.
7. When a result is negative or unexpected, I want that result recorded as evidence, so that the analysis does not silently disappear.
8. When the analysis should branch, I want the Agent to propose a graph change rather than mutate the workflow silently, so that I can approve or reject it.
9. When a tool cannot proceed, I want the system to explain whether this is a technical failure, biological negative evidence, or insufficient input, so that I know what to do next.
10. When the original hypothesis weakens, I want the system to propose related hypotheses with evidence, so that discovery is not constrained by the initial goal.
11. When AgentFlow lacks a needed tool or method, I want it to search existing tools, public repositories, documentation, and literature before proposing new implementation work, so that it does not pretend knowledge it does not have.
12. When AgentFlow proposes a hypothesis, I want it to show supporting evidence, opposing evidence, uncertainty, and validation steps, so that the reasoning remains scientific rather than self-confirming.
13. When I produce a final report, I want the report to include goals, methods, observations, decisions, negative results, and branch rationale, so that the scientific story is auditable.

## 6. Core Product Principles

1. Every step needs a reason, but not every step needs an LLM call.
2. Every step must have deterministic preflight and postflight checks.
3. Agent reasoning should be triggered by evidence, ambiguity, failure, or branch points.
4. The Agent proposes graph patches; it does not directly mutate execution state.
5. Negative results are first-class evidence.
6. Goal drift is allowed, but it must be explicit and justified.
7. Tool coverage should be tiered, not pretended to be complete.
8. AgentFlow should support partial-entry analysis from existing artifacts.
9. AgentFlow should own task state, retry, resume, cache, runtime, logs, artifacts, and observations.
10. Nextflow/Snakemake integration is valuable, but should be optional and subordinate to the AgentFlow evidence graph.
11. AgentFlow should borrow Nextflow's operational discipline, not rebuild its full feature set.
12. AgentFlow should not rely on the Agent's memorized knowledge when evidence can be retrieved, verified, or tested.
13. Tool gaps should trigger a structured research workflow before custom code is written.
14. Hypotheses must be tracked with supporting evidence, contradictory evidence, confidence, and required validation.
15. The system should reward honest uncertainty over fluent but unsupported conclusions.

## 7. Hook Model

AgentFlow should use hooks at every step, but split them into two categories.

### Deterministic Hooks: Always On

These hooks should not require an LLM.

- Input existence check
- Input type check
- Sample identity check
- Parameter schema check
- Environment availability check
- Output existence check
- Output type check
- Basic summary extraction
- Log and artifact registration

### Cognitive Agent Hooks: Conditional

These hooks use an Agent only when the result requires interpretation or a decision.

Trigger conditions:

- Step failed
- Output is empty
- Output contradicts expectation
- QC warning appears
- Batch effect or sample mismatch appears
- Tool branch reaches a dead end
- New candidate signal appears
- User asks for interpretation
- Report section is needed
- The next step is scientifically ambiguous

This avoids both extremes:

- No reasoning until the end
- Expensive, unstable LLM calls before and after every command

## 8. Core Concepts

### Goal

The user's scientific intent, including the initial hypothesis.

Example:

> Evaluate whether gene A is a viable tumor treatment marker.

### Step Contract

A structured contract for a step:

- Why this step is relevant
- Required inputs
- Expected outputs
- Parameters
- Runtime requirements
- Validation checks
- Observer summary
- Possible next decisions

### Observation

A compact structured interpretation of an artifact.

Examples:

- "No high-confidence fusion detected"
- "PC1 is strongly correlated with sample batch"
- "Gene A is not significantly associated with survival"
- "Homolog gene B shows stronger tumor-specific expression"

### Decision

A recorded reason for continuing, stopping, branching, or changing the hypothesis.

### Graph Patch

A proposed mutation to the analysis graph.

Examples:

- Add Harmony branch
- Stop fusion branch as negative evidence
- Add homolog gene exploration
- Add pathway enrichment branch
- Pause for user approval

### Evidence Graph

The durable product object.

It contains:

- Goals
- Steps
- Artifacts
- Observations
- Decisions
- Branches
- Approvals
- Rejections
- Reports

## 9. Tool Coverage Strategy

AgentFlow should not claim to cover every scientific tool.

Tools should have maturity levels.

### Level 1: Verified Tool

Full contract:

- Input schema
- Output schema
- Runtime
- Validator
- Observer
- Known failure modes
- Report template

### Level 2: Wrapped Tool

Basic contract:

- Command wrapper
- Declared inputs/outputs
- Minimal validation
- Basic log capture

### Level 3: Exploratory Tool

Allowed for exploration, but marked low-confidence:

- Requires user approval
- Must preserve command and logs
- Must not overwrite original data
- Must not be promoted to reusable workflow until wrapped or verified

This makes the product honest about tool coverage.

### Active Registry vs Discovery Catalog

AgentFlow should not choose between two bad extremes:

- Do not register every known bioinformatics tool as executable capability.
- Do not force the Agent to search the internet from scratch for every missing method.

The product should use three layers:

1. **Active Tool Registry**: small set of executable, approved, schema-checked tools.
2. **Discovery Catalog**: larger read-only index of known tools, packages, repositories, papers, and methods.
3. **Research Mode**: external search and comparison workflow used when the catalog is insufficient.

The Agent should normally see a short ranked candidate list, not thousands of raw tools.

Active registry tools can run. Discovery catalog entries cannot run until reviewed, installed, wrapped, and promoted.

This protects the product from tool overload, unsafe execution, environment chaos, and hallucinated capabilities while still allowing AgentFlow to discover new methods when research requires it.

### Progressive Tool Disclosure

Tool information should be disclosed progressively.

The Agent should not receive every command, parameter, paper, and repository at once. It should first see:

1. Matching capability categories.
2. Top candidate tool families.
3. Ranked candidate tools with short reasons.
4. Detailed tool contracts only for selected candidates.
5. Runtime and installation plans only after approval.
6. Raw documentation, papers, or source code only when needed for verification or wrapping.

This keeps planning fast, reduces hallucination, and makes tool selection auditable.

### Tool Gap Strategy

When a required capability is missing, AgentFlow should not immediately ask the Agent to improvise code.

The product should use a staged tool gap workflow:

1. Search the local Tool Registry.
2. Search the Discovery Catalog.
3. Search project-provided scripts and previous runs.
4. Search trusted documentation and public methods.
5. Search public repositories or packages.
6. Search literature and method papers.
7. Propose candidate tools or implementation options.
8. Ask for user approval before installing, wrapping, or writing new code.
9. If custom code is needed, create it as a low-maturity exploratory tool first.

This keeps AgentFlow from pretending it already knows every method and gives users a reviewable trail for why a new tool was selected or created.

### Literature Access Reality

AgentFlow must assume many papers are not fully accessible.

The product should never claim to have read a full paper if only the title, abstract, metadata, or citation was available.

Literature access should be tiered:

1. Metadata and abstract.
2. Open-access full text.
3. User-provided PDF or institutional export.
4. Subscription connector, if the user later configures one.
5. Unavailable full text.

If full text is unavailable, AgentFlow may use metadata and abstracts for discovery, but report conclusions must mark the evidence as limited. For method implementation or biological claims, abstract-only evidence should usually trigger a request for full text, a user-provided PDF, or an alternative open-access source.

## 10. Unified Execution Model

AgentFlow should provide its own execution model, even if it later delegates some jobs to Nextflow, Snakemake, shell, Python, R, Docker, Conda, or cloud workers.

The product needs a single place to answer:

- What steps exist?
- Which steps are runnable?
- Which inputs satisfied each step?
- Which steps were imported from existing artifacts?
- Which steps ran locally or through another engine?
- Which runtime environment was used?
- Which steps failed, retried, skipped, or stopped as negative evidence?
- Which artifacts were produced?
- Which observations and decisions came from each artifact?

### Borrow From Nextflow, Do Not Rebuild Nextflow

AgentFlow should borrow the parts of Nextflow that make scientific execution reliable:

- Task graph and ready-step scheduling
- Per-task isolated work directories
- Runtime/environment declaration
- Command/script materialization
- Input staging and output publishing
- Cache/resume semantics
- Retry policy and failure state
- Trace/status/log records
- Resource metadata when available

AgentFlow should not prioritize rebuilding:

- Groovy DSL
- Channel algebra and complex operators
- Full Nextflow module ecosystem
- HPC/cloud schedulers in V1
- Complete Nextflow compatibility
- Pipeline marketplace features

The product goal is a small, durable AgentFlow runtime that can run real tasks and preserve scientific evidence. Compatibility with mature workflow engines can come later.

### Step Entry Modes

Not every analysis starts at the beginning of a canonical pipeline.

AgentFlow should support these entry modes:

1. **Source step**: starts from user-provided raw input, such as FASTQ.
2. **Imported artifact step**: starts from existing BAM, count matrix, H5AD, VCF, BED, peak table, fusion table, image, report, or metadata.
3. **Computed step**: produced by an AgentFlow-managed task.
4. **External run step**: produced by an imported Nextflow/Snakemake/notebook/external command run.
5. **Manual assertion step**: user supplies a result or interpretation that must be marked as manual and lower-confidence.

Imported artifacts are not second-class. They become roots in the evidence graph with their own validation, metadata, hash, and provenance notes.

### Required Task State

Each executable step should track:

- `draft`
- `waiting_for_input`
- `ready`
- `waiting_for_approval`
- `queued`
- `running`
- `completed`
- `completed_with_warning`
- `failed`
- `stopped_negative`
- `skipped`
- `superseded`

The distinction between `failed` and `stopped_negative` is essential.

Example:

- Fusion tool crashes because reference index is missing: `failed`
- Fusion tool runs successfully and finds no credible fusion: `stopped_negative`

### Runtime Requirements

AgentFlow needs the same practical execution concerns as workflow engines:

- working directory
- command/script materialization
- input staging
- output collection
- stdout/stderr logs
- retry policy
- cache/resume
- environment declaration
- runtime backend
- resource usage when available
- status query

Supported runtime backends should be staged:

1. Local process
2. Conda or micromamba
3. Docker
4. Singularity/Apptainer
5. Remote/HPC/cloud adapters
6. Nextflow/Snakemake adapter/import/export

The product should not start with all of these, but the data model must not prevent them.

### Nextflow Relationship

Nextflow remains valuable, but primarily in three roles:

1. **Adapter**: AgentFlow step calls a known Nextflow module/pipeline.
2. **Importer**: AgentFlow imports trace/report/results from a completed Nextflow run.
3. **Exporter**: AgentFlow exports stabilized deterministic branches to Nextflow.

AgentFlow should not require a user to express every scientific path as a Nextflow pipeline before the system can reason about it.

## 11. MVP Scope

The MVP should prove that AgentFlow adds value beyond a notebook or Nextflow trace.

### MVP Must Include

1. Project-level goal capture
2. Flow draft creation
3. Step contracts
4. Tool registry for a small verified tool set
5. DAG construction and ready-step scheduling
6. Local process executor
7. Work directory creation
8. Command/script materialization
9. Runtime/environment declaration
10. Deterministic input/output validation hooks
11. Artifact registry
12. Observation records
13. Decision records
14. Graph patch proposal format
15. User approval/rejection for graph patches
16. Imported artifact roots
17. Minimal task state management
18. Retry/status/log tracking
19. Cache/resume metadata
20. Report generation from evidence graph

### MVP Should Not Include

- HPC scheduling
- Kubernetes
- Large tool marketplace
- Fully autonomous analysis
- Complex visual UI
- Multi-user collaboration
- Automatic publication-ready claims
- Complete Nextflow/Snakemake parity
- Nextflow adapter/import/export as a V1 dependency

## 12. First Validation Scenario

Use one realistic scenario before building broad infrastructure.

Scenario:

> User wants to test whether gene A is a tumor therapy marker.

Expected analysis path:

1. Capture user goal and hypothesis.
2. Generate initial draft analysis.
3. Validate expression/survival/sample metadata inputs.
4. Run or import deterministic analysis results.
5. Observe whether gene A supports the marker hypothesis.
6. If weak, record negative evidence.
7. Propose branches:
   - Homolog genes
   - Interacting genes
   - Upstream/downstream pathway genes
   - Tumor-specific expression
   - Survival association
8. Ask user to approve branch exploration.
9. Generate final report:
   - Original hypothesis
   - Evidence against/for it
   - New candidate hypotheses
   - Why each branch was created or stopped

Success criterion:

> A researcher should read the final evidence graph/report and agree that the analysis path is scientifically understandable, even if the original hypothesis failed.

## 13. Report Requirements

The report should not only summarize results. It must preserve causal reasoning.

Required report sections:

- Original user goal
- Initial hypothesis
- Data inputs and validation status
- Analysis steps and reasons
- Observations
- Negative results
- Branch decisions
- Revised hypotheses
- Final interpretation
- Open questions
- Recommended next experiments

## 14. Product Risks

### Risk 1: It Becomes Only a Workflow Engine

If the product focuses only on scheduling, runtime, and YAML, it loses differentiation.

Mitigation:

Build execution only as the substrate for evidence, observation, and decision tracking.

### Risk 2: It Becomes Chat Logs With Diagrams

If decisions are vague natural language only, it will not be reproducible.

Mitigation:

Use structured observations and graph patches.

### Risk 3: It Overtrusts AI Reasoning

If the Agent silently changes direction, the product becomes unsafe.

Mitigation:

Require explicit approval for hypothesis shifts and branch creation.

### Risk 4: Tool Coverage Is Too Narrow

If only a few tools are supported, users may abandon it.

Mitigation:

Use tiered tool support and allow exploratory tools with clear confidence labels.

### Risk 5: It Adds Friction

If every small action requires approval, users will avoid it.

Mitigation:

Separate deterministic hooks from cognitive approval gates.

### Risk 6: It Splits State Across Workflow Engines

If deterministic tasks live only in Nextflow/Snakemake while Agent decisions live only in AgentFlow, users will lose the causal chain.

Mitigation:

AgentFlow owns project step/run/artifact state. External workflow engines are adapters, importers, or exporters.

### Risk 7: It Defers Execution Too Long

If V1 only creates plans, diagrams, and reports, it will not prove that AgentFlow can manage real scientific work.

Mitigation:

V1 must run real tasks with a small local runtime: task graph, scheduler, workdir, logs, status, retry, cache metadata, artifact validation, and report export.

## 15. Open Product Questions

1. Which standalone CLI-first product shape should V1 use before later Omiga integration?
2. What is the first domain: scRNA, bulk RNA-seq, tumor marker discovery, fusion analysis, or general omics?
3. What should require user approval by default?
4. What counts as a meaningful observation?
5. How much negative evidence should be surfaced in the final report?
6. Should V1 support only local process execution, or local process plus Conda/micromamba from day one?
7. How should revised hypotheses be ranked?
8. What level of tool wrapping is acceptable for the first release?
9. Which intermediate artifact types must be accepted as graph roots in the first release?

## 16. Proposed Development Sequence

### Phase 1: Minimal Runnable Foundation

Build the smallest AgentFlow runtime that can execute real steps:

- Project state store
- Tool registry
- Step contract schema
- DAG builder
- Ready-step scheduler
- Local process executor
- Work directory layout
- Command/script materialization
- Input staging
- Output collection
- Logs
- Status query
- Retry record
- Cache/resume metadata
- Artifact registry

Goal:

Prove AgentFlow can manage real tasks without depending on Nextflow.

### Phase 2: Evidence Graph and Report Prototype

Add:

- Goal
- Step
- Artifact
- Observation
- Decision
- Branch
- Approval
- Imported artifact roots
- Report export

### Phase 3: Validator and Observer Layer

Add deterministic hooks for selected tools.

### Phase 4: Agent Graph Patch Layer

Add Agent-generated branch proposals with approval.

### Phase 5: Execution Integration

Add Conda/micromamba support first, then Docker if needed.

### Phase 6: Broader Runtime Support

Only after V1 is useful, add Singularity/Apptainer, remote/HPC/cloud adapters, and Nextflow/Snakemake import/export.

## 17. Review Checklist

Before writing code, reviewers should decide:

1. Is the product thesis correct?
2. Is AgentFlow a unified evidence graph and task state manager, rather than only a workflow engine?
3. Which first domain should validate the idea?
4. Which decisions must be approved by the user?
5. What should the first report look like?
6. Which parts are too abstract or too ambitious?
7. Which intermediate input types should be first-class roots?
8. Does V1 run real tasks without requiring Nextflow?
9. Is the runtime small enough to build quickly but complete enough to manage status, retry, logs, cache metadata, and artifacts?
