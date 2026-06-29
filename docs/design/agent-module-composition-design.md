# Design: agent autonomously composes registered modules (slice 4b)

Goal: when the agent builds a flow to answer a hypothesis, it may select a
registered **module** (a reusable typed sub-flow) — not just a single tool — as
the producer of a needed artifact type, and inline-expand it into the flow.
`argument.rs` (the 0-LLM verdict engine) is untouched; this is all in the
composition layer.

## The two worlds it must bridge

1. **Agent composition** works in `ProposedStep` (branch.rs): `{ id, tool,
   needs, inputs: Vec<(name,value)>, params, outputs: Vec<(name,value)> }`.
   - `draft_step_for` names a step's outputs `"{step_id}_{port}"`.
   - `chain_producer_steps_rec` finds a *tool* whose output port type matches a
     consumer's missing input type, drafts it, recurses, and wires the consumer
     input to `"{producer_step_id}.{output_port}"`.
   - `needs` are then `infer_step_needs`'d (from real artifacts' `source_step_id`)
     and the apply/graph-patch path resolves the `"stepid.port"` input refs into
     flow edges.
2. **Modules** expand (`ModuleSpec::expand`) into `FlowStepDraft`s with
   *artifact-name* wiring: internal steps reference each other by artifact name
   (`"{instance}__{artifact}"`), and external output ports are exposed as
   `"{instance}__{from}"`. This is the flow's native wiring (a consumer input =
   an artifact name produced by an upstream step + an explicit `needs`).

So the integration must turn a chosen module into a set of `ProposedStep`s and
decide how the consumer references the module's output.

## Wiring decision

A module's expanded steps keep their **artifact-name** wiring (it is already
flow-native and self-consistent per instance). The consumer's missing input is
wired to the module's **exposed output artifact name** (`"{instance}__{from}"`),
and the consumer gains an explicit `needs` on the module's terminal internal
step. Because these producers are not yet real artifacts, the converter sets
`ProposedStep.needs` **explicitly** (as `chain_producer_steps` already does for
its prerequisite producer steps) rather than relying on `infer_step_needs`.

This keeps modules orthogonal to the `"stepid.port"` tool convention: both
resolve through the existing flow edge/needs model at apply time.

## Injection points & phases

- **4b-1 — discovery (THIS slice).** `ProjectStore::match_modules(desired_output_type,
  available_input_types) -> Vec<ModuleCandidate>`: scan registered modules, keep
  those with an output port of the desired type, score by how many input ports
  are already available (mirrors `match_tools`' Fit). Pure, no agent-loop change,
  fully testable. Nothing calls it yet (it is the primitive 4b-2 composes).
- **4b-2 — expansion as a producer.** In `chain_producer_steps_rec`, after the
  tool candidates, also consider `match_modules` candidates that are `Fit::High`
  (every module input port already available — no chaining *into* a module yet).
  On selection: bind input ports to available artifacts by type, `expand` the
  module, convert each `FlowStepDraft` to a `ProposedStep`, wire the consumer
  input to the exposed output + add the `needs`, and return the converted steps
  as prerequisites. Gate behind High-fit so the producer is atomic.
- **4b-3 — answer-level matching & brake interplay.** Let a module be a
  top-level answer candidate in `enrich_branch_proposal`, and teach the
  equivalent-branch brake about module instances so it does not mistake a
  module's internal steps for alternative answers.
- **4b-4 — deeper.** Chain *into* a module's unmet input ports; nested modules
  (module-in-module); module maturity/scoring.

## Constraints / invariants

- `argument.rs` byte-identical.
- Module instance ids must be unique within a flow (the converter derives a
  fresh instance id per selection, like `draft_step_for`'s `step_…`).
- A module is only a *producer* candidate (it must produce the desired type); the
  agent never silently runs a module whose outputs are unused.
- Nested modules remain unsupported until 4b-4.
