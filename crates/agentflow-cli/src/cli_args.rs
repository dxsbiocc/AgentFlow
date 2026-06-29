use std::ffi::OsString;
use std::path::PathBuf;

use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand};

use crate::{usage, CliError};

pub(crate) fn run<I>(args: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let argv = args.into_iter().collect::<Vec<_>>();
    let Some(first) = argv.get(1) else {
        return Ok(usage());
    };
    if matches!(first.to_str(), Some("--help" | "-h" | "help")) {
        return Ok(usage());
    }
    if matches!(first.to_str(), Some("--version" | "-V" | "version")) {
        return Ok(agentflow_core::version_line());
    }

    match Cli::try_parse_from(argv) {
        Ok(cli) => dispatch(cli),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            Ok(error.to_string().trim_end().to_string())
        }
        Err(error) => Err(CliError::InvalidArgument(
            error.to_string().trim_end().to_string(),
        )),
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "agentflow",
    about = "CLI-first local runtime for AgentFlow",
    disable_help_subcommand = true,
    disable_version_flag = true
)]
struct Cli {
    #[command(subcommand)]
    command: TopCommand,
}

#[derive(Debug, Subcommand)]
enum TopCommand {
    Init(InitArgs),
    Status(PathJsonArgs),
    Doctor(PathJsonArgs),
    Tools(ToolsArgs),
    Synth(SynthArgs),
    Llm(LlmArgs),
    Env(EnvArgs),
    Import(ImportArgs),
    Artifacts(ArtifactsArgs),
    Module(ModuleArgs),
    Flow(FlowArgs),
    Run(RunArgs),
    #[command(name = "run-step")]
    RunStep(StepRefArgs),
    Report(ReportArgs),
    Cache(CacheArgs),
    Retry(StepRefArgs),
    Observe(ObserveArgs),
    Observations(ObservationsArgs),
    Research(ResearchArgs),
    Agent(AgentArgs),
    Hypothesis(HypothesisArgs),
    Evidence(EvidenceArgs),
    Verdict(VerdictArgs),
    Branch(BranchArgs),
    Decision(DecisionArgs),
    Forage(ForageArgs),
    Trace(TraceArgs),
    Patch(PatchArgs),
    Compare(CompareArgs),
    Runs(RunsArgs),
    Logs(LogsArgs),
}

#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    #[arg(long, value_name = "name")]
    pub(crate) name: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct PathOnlyArgs {
    #[arg(long, value_name = "path")]
    pub(crate) path: Vec<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct PathJsonArgs {
    #[arg(long, value_name = "path")]
    pub(crate) path: Vec<PathBuf>,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct RunArgs {
    #[arg(value_name = "flow-id")]
    pub(crate) flow_id: String,
    #[arg(long, value_name = "docker|podman|singularity|apptainer")]
    pub(crate) container_engine: Vec<String>,
    #[arg(long, value_name = "path")]
    pub(crate) container_runner: Vec<PathBuf>,
    #[arg(long, value_name = "n")]
    pub(crate) max_parallel: Vec<usize>,
    #[arg(long)]
    pub(crate) keep_going: bool,
    #[arg(long, value_name = "n")]
    pub(crate) retries: Vec<usize>,
    #[arg(long, value_name = "seconds")]
    pub(crate) retry_backoff: Vec<usize>,
    #[command(flatten)]
    pub(crate) project: PathOnlyArgs,
}

#[derive(Debug, Args)]
pub(crate) struct StepRefArgs {
    #[arg(value_name = "step-id")]
    pub(crate) step_id: String,
    #[command(flatten)]
    pub(crate) project: PathOnlyArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ReportArgs {
    #[arg(value_name = "flow-id")]
    pub(crate) flow_id: String,
    #[command(flatten)]
    pub(crate) project: PathOnlyArgs,
}

#[derive(Debug, Args)]
pub(crate) struct LogsArgs {
    #[arg(value_name = "run-or-attempt-id")]
    pub(crate) id: String,
    #[command(flatten)]
    pub(crate) project: PathOnlyArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ToolsArgs {
    #[command(subcommand)]
    pub(crate) command: ToolsCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ToolsCommand {
    Register(ToolsRegisterArgs),
    Supersede(ToolsSupersedeArgs),
    List(PathJsonArgs),
    Inspect(ToolsInspectArgs),
    Match(ToolsMatchArgs),
    #[command(name = "draft-step")]
    DraftStep(ToolsDraftStepArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ToolsRegisterArgs {
    #[arg(value_name = "tool.yaml")]
    pub(crate) tool_yaml: PathBuf,
    #[command(flatten)]
    pub(crate) project: PathOnlyArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ToolsSupersedeArgs {
    #[arg(value_name = "old-tool-ref")]
    pub(crate) old_tool_ref: String,
    #[arg(long = "by", value_name = "new-tool-ref")]
    pub(crate) successor_tool_ref: String,
    #[arg(long, value_name = "text")]
    pub(crate) reason: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ToolsInspectArgs {
    #[arg(value_name = "tool-ref")]
    pub(crate) tool_ref: String,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ToolsMatchArgs {
    #[arg(long, value_name = "type")]
    pub(crate) output: Vec<String>,
    #[arg(long, value_name = "type")]
    pub(crate) input: Vec<String>,
    #[arg(long, value_name = "kw")]
    pub(crate) keyword: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ToolsDraftStepArgs {
    #[arg(value_name = "tool-ref")]
    pub(crate) tool_ref: String,
    #[arg(long, value_name = "type:artifact-id")]
    pub(crate) input: Vec<String>,
    #[arg(long, value_name = "id")]
    pub(crate) hypothesis: Vec<String>,
    #[arg(long)]
    pub(crate) infer_params: bool,
    #[arg(long, value_name = "cmd")]
    pub(crate) synthesizer: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct SynthArgs {
    #[arg(long, value_name = "n")]
    pub(crate) name: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) description: Vec<String>,
    #[arg(long, value_name = "input-file")]
    pub(crate) fixture: Vec<PathBuf>,
    #[arg(long, value_name = "substring")]
    pub(crate) expect: Vec<String>,
    #[arg(long, value_name = "cmd")]
    pub(crate) synthesizer: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathOnlyArgs,
}

#[derive(Debug, Args)]
pub(crate) struct EnvArgs {
    #[command(subcommand)]
    pub(crate) command: EnvCommand,
}

#[derive(Debug, Args)]
pub(crate) struct LlmArgs {
    #[command(subcommand)]
    pub(crate) command: LlmCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum LlmCommand {
    Config(LlmConfigArgs),
}

#[derive(Debug, Args)]
pub(crate) struct LlmConfigArgs {
    #[arg(long, value_name = "anthropic|openai|gemini|deepseek")]
    pub(crate) provider: Vec<String>,
    #[arg(long, value_name = "key")]
    pub(crate) api_key: Vec<String>,
    #[arg(long, value_name = "env-var")]
    pub(crate) api_key_env: Vec<String>,
    #[arg(long, value_name = "model")]
    pub(crate) model: Vec<String>,
    #[arg(long, value_name = "url")]
    pub(crate) base_url: Vec<String>,
    #[arg(long, value_name = "cmd")]
    pub(crate) synthesizer: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Subcommand)]
pub(crate) enum EnvCommand {
    Check(ToolRefJsonArgs),
    Prepare(ToolRefJsonArgs),
    Export(ToolRefJsonArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ToolRefJsonArgs {
    #[arg(value_name = "tool-ref")]
    pub(crate) tool_ref: String,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ImportArgs {
    #[arg(value_name = "file")]
    pub(crate) file: PathBuf,
    #[arg(long = "type", value_name = "artifact-type")]
    pub(crate) artifact_type: Vec<String>,
    #[arg(long, value_name = "reference|copy")]
    pub(crate) mode: Vec<String>,
    #[arg(long)]
    pub(crate) allow_external_reference: bool,
    #[command(flatten)]
    pub(crate) project: PathOnlyArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ArtifactsArgs {
    #[command(subcommand)]
    pub(crate) command: ArtifactsCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ArtifactsCommand {
    List(PathJsonArgs),
    Inspect(ArtifactInspectArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ArtifactInspectArgs {
    #[arg(value_name = "artifact-id")]
    pub(crate) artifact_id: String,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ModuleArgs {
    #[command(subcommand)]
    pub(crate) command: ModuleCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ModuleCommand {
    Register(ModuleRegisterArgs),
    List(ModuleListArgs),
    Validate(ModuleFileArgs),
    Show(ModuleFileArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ModuleRegisterArgs {
    #[arg(value_name = "module.yaml")]
    pub(crate) module_yaml: PathBuf,
    #[command(flatten)]
    pub(crate) project: PathOnlyArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ModuleListArgs {
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ModuleFileArgs {
    #[arg(value_name = "file")]
    pub(crate) path: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct FlowArgs {
    #[command(subcommand)]
    pub(crate) command: FlowCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum FlowCommand {
    Validate(FlowValidateArgs),
    Approve(FlowApproveArgs),
    Inspect(FlowInspectArgs),
    Plan(FlowInspectArgs),
}

#[derive(Debug, Args)]
pub(crate) struct FlowValidateArgs {
    #[arg(value_name = "flow.yaml")]
    pub(crate) flow_yaml: PathBuf,
    #[arg(long, value_name = "path")]
    pub(crate) module: Vec<PathBuf>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct FlowApproveArgs {
    #[arg(value_name = "flow.yaml")]
    pub(crate) flow_yaml: PathBuf,
    #[arg(long, value_name = "path")]
    pub(crate) module: Vec<PathBuf>,
    #[command(flatten)]
    pub(crate) project: PathOnlyArgs,
}

#[derive(Debug, Args)]
pub(crate) struct FlowInspectArgs {
    #[arg(value_name = "flow-id")]
    pub(crate) flow_id: String,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct CacheArgs {
    #[command(subcommand)]
    pub(crate) command: CacheCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CacheCommand {
    Explain(CacheExplainArgs),
    List(PathJsonArgs),
    Prune(CachePruneArgs),
}

#[derive(Debug, Args)]
pub(crate) struct CacheExplainArgs {
    #[arg(value_name = "flow-id|step-id")]
    pub(crate) target: String,
    #[command(flatten)]
    pub(crate) project: PathOnlyArgs,
}

#[derive(Debug, Args)]
pub(crate) struct CachePruneArgs {
    #[arg(long)]
    pub(crate) all: bool,
    #[arg(long, value_name = "seconds")]
    pub(crate) older_than_seconds: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ObserveArgs {
    #[arg(value_name = "artifact-id")]
    pub(crate) artifact_id: String,
    #[arg(long, value_name = "artifact_summary|marker_report")]
    pub(crate) adapter: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ObservationsArgs {
    #[command(subcommand)]
    pub(crate) command: ObservationsCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ObservationsCommand {
    List(PathJsonArgs),
    Inspect(ObservationInspectArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ObservationInspectArgs {
    #[arg(value_name = "observation-id")]
    pub(crate) observation_id: String,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ResearchArgs {
    #[command(subcommand)]
    pub(crate) command: ResearchCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ResearchCommand {
    Note(ResearchNoteArgs),
    List(PathJsonArgs),
    Inspect(ResearchInspectArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ResearchNoteArgs {
    #[arg(long, value_name = "text")]
    pub(crate) problem: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) question: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) finding: Vec<String>,
    #[arg(long, value_name = "low|medium|high")]
    pub(crate) confidence: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) source: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathOnlyArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ResearchInspectArgs {
    #[arg(value_name = "note-id")]
    pub(crate) note_id: String,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct HypothesisArgs {
    #[command(subcommand)]
    pub(crate) command: HypothesisCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum HypothesisCommand {
    Create(HypothesisCreateArgs),
    List(PathJsonArgs),
    Show(HypothesisShowArgs),
    Transition(HypothesisTransitionArgs),
}

#[derive(Debug, Args)]
pub(crate) struct HypothesisCreateArgs {
    #[arg(long, value_name = "text")]
    pub(crate) statement: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) origin: Vec<String>,
    #[arg(long, value_name = "goal-id")]
    pub(crate) goal: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct HypothesisShowArgs {
    #[arg(value_name = "hypothesis-id")]
    pub(crate) hypothesis_id: String,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct HypothesisTransitionArgs {
    #[arg(value_name = "hypothesis-id")]
    pub(crate) hypothesis_id: String,
    #[arg(long, value_name = "status")]
    pub(crate) to: Vec<String>,
    #[arg(long, value_name = "low|medium|high")]
    pub(crate) confidence: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct EvidenceArgs {
    #[command(subcommand)]
    pub(crate) command: EvidenceCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum EvidenceCommand {
    Link(EvidenceLinkArgs),
    List(EvidenceListArgs),
}

#[derive(Debug, Args)]
pub(crate) struct EvidenceLinkArgs {
    #[arg(long, value_name = "id")]
    pub(crate) hypothesis: Vec<String>,
    #[arg(long, value_name = "obs-id")]
    pub(crate) observation: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) source: Vec<String>,
    #[arg(long, value_name = "grade")]
    pub(crate) grade: Vec<String>,
    #[arg(long, value_name = "supports|contradicts|neutral")]
    pub(crate) stance: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) note: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct EvidenceListArgs {
    #[arg(long, value_name = "id")]
    pub(crate) hypothesis: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct VerdictArgs {
    #[command(subcommand)]
    pub(crate) command: VerdictCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum VerdictCommand {
    Render(VerdictRenderArgs),
    Show(VerdictShowArgs),
}

#[derive(Debug, Args)]
pub(crate) struct VerdictRenderArgs {
    #[arg(long, value_name = "id")]
    pub(crate) hypothesis: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) gate_supports: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) gate_against: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) gate_alternatives: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) gate_data_risks: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) gate_assumptions: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) gate_falsifier: Vec<String>,
    #[arg(long, value_name = "observed|inferred|speculative")]
    pub(crate) gate_claim_basis: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) gate_not_yet: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct VerdictShowArgs {
    #[arg(long, value_name = "id")]
    pub(crate) hypothesis: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct AgentArgs {
    #[arg(
        long,
        global = true,
        value_name = "docker|podman|singularity|apptainer"
    )]
    pub(crate) container_engine: Vec<String>,
    #[arg(long, global = true, value_name = "path")]
    pub(crate) container_runner: Vec<PathBuf>,
    #[arg(long, global = true, value_name = "n")]
    pub(crate) max_parallel: Vec<usize>,
    #[arg(long, global = true)]
    pub(crate) keep_going: bool,
    #[arg(long, global = true, value_name = "n")]
    pub(crate) retries: Vec<usize>,
    #[arg(long, global = true, value_name = "seconds")]
    pub(crate) retry_backoff: Vec<usize>,
    #[command(subcommand)]
    pub(crate) command: AgentCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AgentCommand {
    Run(AgentRunArgs),
}

#[derive(Debug, Args)]
pub(crate) struct AgentRunArgs {
    #[arg(long)]
    pub(crate) apply: bool,
    #[arg(long)]
    pub(crate) no_apply: bool,
    #[arg(long)]
    pub(crate) auto_run: bool,
    #[arg(long)]
    pub(crate) no_auto_run: bool,
    #[arg(long)]
    pub(crate) dry_run: bool,
    #[arg(long, value_name = "flow-id")]
    pub(crate) flow: Vec<String>,
    #[arg(long, value_name = "n")]
    pub(crate) max_apply: Vec<String>,
    #[arg(long, value_name = "n")]
    pub(crate) max_chain_depth: Vec<String>,
    #[arg(long)]
    pub(crate) propose_synth: bool,
    #[arg(long)]
    pub(crate) auto_synth: bool,
    #[arg(long)]
    pub(crate) no_auto_synth: bool,
    #[arg(long)]
    pub(crate) infer_params: bool,
    #[arg(long)]
    pub(crate) no_infer_params: bool,
    #[arg(long)]
    pub(crate) semantic_match: bool,
    #[arg(long)]
    pub(crate) no_semantic_match: bool,
    #[arg(long, value_name = "cmd")]
    pub(crate) synthesizer: Vec<String>,
    #[arg(long)]
    pub(crate) auto_forage: bool,
    #[arg(long)]
    pub(crate) no_auto_forage: bool,
    #[arg(long, value_name = "n")]
    pub(crate) forage_max: Vec<String>,
    #[arg(long, value_name = "path")]
    pub(crate) forage_script: Vec<PathBuf>,
    #[arg(long, value_name = "bin")]
    pub(crate) python: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct BranchArgs {
    #[command(subcommand)]
    pub(crate) command: BranchCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum BranchCommand {
    Candidates(PathJsonArgs),
    Select(BranchSelectArgs),
}

#[derive(Debug, Args)]
pub(crate) struct BranchSelectArgs {
    #[arg(long)]
    pub(crate) explore: bool,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct DecisionArgs {
    #[command(subcommand)]
    pub(crate) command: DecisionCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum DecisionCommand {
    List(PathJsonArgs),
    Pending(PathJsonArgs),
    Show(DecisionShowArgs),
    Resolve(DecisionResolveArgs),
}

#[derive(Debug, Args)]
pub(crate) struct DecisionShowArgs {
    #[arg(value_name = "decision-id")]
    pub(crate) decision_id: String,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct DecisionResolveArgs {
    #[arg(value_name = "decision-id")]
    pub(crate) decision_id: String,
    #[arg(long, value_name = "index")]
    pub(crate) choose: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) note: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ForageArgs {
    #[command(subcommand)]
    pub(crate) command: ForageCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ForageCommand {
    Observe(ForageObserveArgs),
    Ingest(ForageIngestArgs),
    Fetch(ForageFetchArgs),
    List(PathJsonArgs),
    Show(ForageShowArgs),
    Link(ForageLinkArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ForageObserveArgs {
    #[arg(long, value_name = "source")]
    pub(crate) source: Vec<String>,
    #[arg(long, value_name = "external-id")]
    pub(crate) external_id: Vec<String>,
    #[arg(long, value_name = "title")]
    pub(crate) title: Vec<String>,
    #[arg(long, value_name = "access")]
    pub(crate) access: Vec<String>,
    #[arg(long)]
    pub(crate) retracted: bool,
    #[arg(long, value_name = "published-id")]
    pub(crate) published_as: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ForageIngestArgs {
    #[arg(value_name = "hits-file")]
    pub(crate) hits_file: PathBuf,
    #[arg(long, value_name = "source")]
    pub(crate) source: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ForageFetchArgs {
    #[arg(long, value_name = "query")]
    pub(crate) query: Vec<String>,
    #[arg(long, value_name = "source")]
    pub(crate) source: Vec<String>,
    #[arg(long, value_name = "path")]
    pub(crate) script: Vec<PathBuf>,
    #[arg(long, value_name = "n")]
    pub(crate) max: Vec<String>,
    #[arg(long, value_name = "bin")]
    pub(crate) python: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ForageShowArgs {
    #[arg(value_name = "forage-obs-id")]
    pub(crate) forage_obs_id: String,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ForageLinkArgs {
    #[arg(long, value_name = "id")]
    pub(crate) hypothesis: Vec<String>,
    #[arg(long, value_name = "forage-obs-id")]
    pub(crate) observation: Vec<String>,
    #[arg(long, value_name = "supports|contradicts|neutral")]
    pub(crate) stance: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) note: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct TraceArgs {
    #[command(subcommand)]
    pub(crate) command: TraceCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum TraceCommand {
    Checkpoint(TraceCheckpointArgs),
    List(PathJsonArgs),
    Drift(TraceCheckpointIdArgs),
    Revert(TraceCheckpointIdArgs),
}

#[derive(Debug, Args)]
pub(crate) struct TraceCheckpointArgs {
    #[arg(long, value_name = "text")]
    pub(crate) label: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct TraceCheckpointIdArgs {
    #[arg(value_name = "checkpoint-id")]
    pub(crate) checkpoint_id: String,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct PatchArgs {
    #[command(subcommand)]
    pub(crate) command: PatchCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PatchCommand {
    Propose(PatchProposeArgs),
    List(PatchListArgs),
    Approve(PatchIdArgs),
    Reject(PatchRejectArgs),
    Apply(PatchIdArgs),
}

#[derive(Debug, Args)]
pub(crate) struct PatchProposeArgs {
    #[arg(value_name = "flow-id")]
    pub(crate) flow_id: String,
    #[arg(long, value_name = "text")]
    pub(crate) title: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) reason: Vec<String>,
    #[arg(long, value_name = "json")]
    pub(crate) patch_json: Vec<String>,
    #[arg(long, value_name = "file")]
    pub(crate) patch_file: Vec<PathBuf>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct PatchListArgs {
    #[arg(value_name = "flow-id")]
    pub(crate) flow_id: String,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct PatchIdArgs {
    #[arg(value_name = "patch-id")]
    pub(crate) patch_id: String,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct PatchRejectArgs {
    #[arg(value_name = "patch-id")]
    pub(crate) patch_id: String,
    #[arg(long, value_name = "text")]
    pub(crate) reason: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct CompareArgs {
    #[command(subcommand)]
    pub(crate) command: CompareCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CompareCommand {
    Steps(CompareStepsArgs),
    Metrics(CompareMetricsArgs),
    List(CompareListArgs),
    Inspect(CompareInspectArgs),
}

#[derive(Debug, Args)]
pub(crate) struct CompareStepsArgs {
    #[arg(value_name = "flow-id")]
    pub(crate) flow_id: String,
    #[arg(long, value_name = "step-id")]
    pub(crate) baseline: Vec<String>,
    #[arg(long, value_name = "step-id")]
    pub(crate) candidate: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) summary: Vec<String>,
    #[arg(long, value_name = "baseline|candidate|tie|inconclusive")]
    pub(crate) winner: Vec<String>,
    #[arg(long, value_name = "text")]
    pub(crate) reason: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct CompareMetricsArgs {
    #[arg(value_name = "flow-id")]
    pub(crate) flow_id: String,
    #[arg(long, value_name = "step-id")]
    pub(crate) baseline: Vec<String>,
    #[arg(long, value_name = "step-id")]
    pub(crate) candidate: Vec<String>,
    #[arg(long, value_name = "name")]
    pub(crate) metric: Vec<String>,
    #[arg(long, value_name = "higher|lower")]
    pub(crate) direction: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct CompareListArgs {
    #[arg(value_name = "flow-id")]
    pub(crate) flow_id: String,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct CompareInspectArgs {
    #[arg(value_name = "comparison-id")]
    pub(crate) comparison_id: String,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct RunsArgs {
    #[command(subcommand)]
    pub(crate) command: RunsCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum RunsCommand {
    List(RunsListArgs),
    Inspect(RunsInspectArgs),
}

#[derive(Debug, Args)]
pub(crate) struct RunsListArgs {
    #[arg(long, value_name = "flow-id")]
    pub(crate) flow: Vec<String>,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

#[derive(Debug, Args)]
pub(crate) struct RunsInspectArgs {
    #[arg(value_name = "run-or-attempt-id")]
    pub(crate) id: String,
    #[command(flatten)]
    pub(crate) project: PathJsonArgs,
}

fn dispatch(cli: Cli) -> Result<String, CliError> {
    match cli.command {
        TopCommand::Init(args) => crate::init_command(args),
        TopCommand::Status(args) => crate::status_command(args),
        TopCommand::Doctor(args) => crate::doctor_command(args),
        TopCommand::Tools(args) => crate::tools_command(args),
        TopCommand::Synth(args) => crate::synth_commands::synth_command(args),
        TopCommand::Llm(args) => crate::llm_commands::llm_command(args),
        TopCommand::Env(args) => crate::env_command(args),
        TopCommand::Import(args) => crate::import_command(args),
        TopCommand::Artifacts(args) => crate::artifacts_command(args),
        TopCommand::Module(args) => crate::module_command(args),
        TopCommand::Flow(args) => crate::flow_command(args),
        TopCommand::Run(args) => crate::run_command(args),
        TopCommand::RunStep(args) => crate::run_step_command(args),
        TopCommand::Report(args) => crate::report_command(args),
        TopCommand::Cache(args) => crate::cache_command(args),
        TopCommand::Retry(args) => crate::retry_command(args),
        TopCommand::Observe(args) => crate::observe_command(args),
        TopCommand::Observations(args) => crate::observations_command(args),
        TopCommand::Research(args) => crate::research_command(args),
        TopCommand::Agent(args) => crate::agent_command(args),
        TopCommand::Hypothesis(args) => crate::agent_commands::hypothesis_command(args),
        TopCommand::Evidence(args) => crate::agent_commands::evidence_command(args),
        TopCommand::Verdict(args) => crate::agent_commands::verdict_command(args),
        TopCommand::Branch(args) => crate::agent_ops_commands::branch_command(args),
        TopCommand::Decision(args) => crate::agent_ops_commands::decision_command(args),
        TopCommand::Forage(args) => crate::agent_ops_commands::forage_command(args),
        TopCommand::Trace(args) => crate::agent_ops_commands::trace_command(args),
        TopCommand::Patch(args) => crate::patch_command(args),
        TopCommand::Compare(args) => crate::compare_command(args),
        TopCommand::Runs(args) => crate::runs_command(args),
        TopCommand::Logs(args) => crate::logs_command(args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `run --retries N` and `agent run --retries N` (a global flag) both parse
    // into the retries field, so the value reaches the run configuration.
    #[test]
    fn run_parses_retries_flag() {
        let cli = Cli::try_parse_from(["agentflow", "run", "flow-1", "--retries", "3"])
            .expect("run --retries should parse");
        let TopCommand::Run(args) = cli.command else {
            panic!("expected the run subcommand");
        };
        assert_eq!(args.retries, vec![3]);
    }

    #[test]
    fn agent_run_parses_retries_global_flag() {
        let cli = Cli::try_parse_from(["agentflow", "agent", "run", "--apply", "--retries", "2"])
            .expect("agent run --retries should parse");
        let TopCommand::Agent(agent) = cli.command else {
            panic!("expected the agent subcommand");
        };
        assert_eq!(agent.retries, vec![2]);
        assert!(matches!(agent.command, AgentCommand::Run(_)));
    }

    #[test]
    fn retries_flag_rejects_non_numeric() {
        assert!(Cli::try_parse_from(["agentflow", "run", "flow-1", "--retries", "abc"]).is_err());
    }

    #[test]
    fn module_validate_parses_file_path() {
        let cli = Cli::try_parse_from(["agentflow", "module", "validate", "m.yaml"])
            .expect("module validate should parse");
        let TopCommand::Module(args) = cli.command else {
            panic!("expected the module subcommand");
        };
        let ModuleCommand::Validate(args) = args.command else {
            panic!("expected the validate subcommand");
        };
        assert_eq!(args.path, PathBuf::from("m.yaml"));
    }
}
