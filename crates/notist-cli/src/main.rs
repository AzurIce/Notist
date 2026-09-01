use std::io::IsTerminal;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use notist_analysis::resolve_vault_root;
use notist_service::protocol::ClientKind;
use notist_service::{CoreRequest, CoreResponse, ProtocolViewKind};

mod build;
mod logging;
mod lsp;
mod official_docs;
mod preview;
mod resources;
mod service;
mod skill;

/// Grouped command map mirroring the 2026-08-29 ruling on the command surface:
/// `inspect` is the investigation entry point, the rest is explicit.
const COMMAND_GROUPS: &str = "\
Command groups:
  inspect:     status, ls, search, locate, items, read, ancestors, references, definition
  validate:    check
  maintenance: index
  runtime:     daemon, lsp
  publishing:  build, preview
  meta:        skill

`inspect --help` lists the investigation commands; check validates the whole Vault; the other groups are explicit runtime, maintenance, or publishing actions.";

#[derive(Debug, Parser)]
#[command(
    name = "notist",
    version,
    about,
    arg_required_else_help = true,
    after_help = COMMAND_GROUPS
)]
struct Cli {
    /// Control colored diagnostic output.
    #[arg(long, value_enum, default_value_t = clap::ColorChoice::Auto, global = true)]
    color: clap::ColorChoice,

    /// Run the application service in this process instead of using the local daemon.
    #[arg(long, global = true)]
    no_daemon: bool,

    /// Root directory (or any path inside) of the Vault to operate on.
    #[arg(long, value_name = "DIR", default_value = ".", global = true)]
    vault: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Investigate a Vault: status, ls, search, locate, items, read, ancestors, references, definition.
    #[command(display_order = 1)]
    Inspect {
        #[command(subcommand)]
        command: InspectCommand,
    },
    /// Check module paths and references in a Notist workspace.
    #[command(display_order = 2)]
    Check {
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        summary: bool,
        #[arg(long, value_enum, default_value_t)]
        severity: DiagnosticSeverityArg,
    },
    /// Inspect or rebuild the derived lexical search index.
    #[command(display_order = 3)]
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },
    /// Run the shared local Notist daemon for one vault, or stop the running one.
    #[command(display_order = 4)]
    Daemon {
        #[command(subcommand)]
        action: Option<DaemonAction>,
        #[arg(long, hide = true)]
        background_child: bool,
    },
    /// Run the Notist language server over standard input and output.
    #[command(display_order = 5)]
    Lsp,
    /// Create resources that teach an Agent how to use Notist.
    #[command(display_order = 6)]
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Build a Notist workspace as a multi-page static HTML site.
    #[command(display_order = 7)]
    Build {
        /// Directory to write the generated site.
        #[arg(short, long, default_value = "dist")]
        output: PathBuf,
        /// Remove the selected output directory before writing this build.
        #[arg(long)]
        clean: bool,
    },
    /// Preview a Notist workspace in a local browser with live reload.
    #[command(display_order = 8)]
    Preview {
        /// Network interface on which the preview server listens.
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        /// TCP port. Zero asks the operating system for an available port.
        #[arg(long, default_value_t = 3250)]
        port: u16,
        /// Open the preview URL in the default browser.
        #[arg(long)]
        open: bool,
    },
}

#[derive(Debug, Subcommand)]
enum InspectCommand {
    /// Show a compact Vault, snapshot, diagnostics, and index summary.
    Status,
    /// List the child modules of one module (ls for the Module tree).
    Ls {
        /// Target ModulePath; defaults to the Vault root.
        target: Option<String>,
        /// List the whole subtree instead of direct children only.
        #[arg(long)]
        recursive: bool,
        #[arg(long, value_enum, default_value_t)]
        kind: ModuleKindArg,
    },
    /// Search captured source context in a vault.
    #[command(
        after_help = "Examples:\n  notist inspect search \"workspace snapshot\" docs\n  notist inspect search --exact \"WorkspaceSnapshot\" docs --group-by match\n  notist inspect search --fuzzy \"WorkspaceSnaphot\" docs\n\nLexical/fuzzy search groups by source by default; exact/regex returns each match.\nResults are complete; a zero-hit output proves absence for the selected scope."
    )]
    Search {
        /// Natural-language terms, an identifier, or a literal/regex pattern selected by mode.
        query: String,
        /// Choose ranked lexical search, literal matching, typo-tolerant search, or regex.
        #[arg(long, value_enum, default_value_t)]
        mode: SearchModeArg,
        /// Use literal substring matching; shorthand for `--mode exact`.
        #[arg(long, conflicts_with_all = ["mode", "fuzzy", "regex"])]
        exact: bool,
        /// Use bounded typo expansion; shorthand for `--mode fuzzy`.
        #[arg(long, conflicts_with_all = ["mode", "exact", "regex"])]
        fuzzy: bool,
        /// Use a Rust-compatible regular expression; shorthand for `--mode regex`.
        #[arg(long, conflicts_with_all = ["mode", "exact", "fuzzy"])]
        regex: bool,
        /// Restrict matches to an exact ModulePath prefix; may be repeated.
        #[arg(long = "scope")]
        scopes: Vec<String>,
        /// Search only selected authored/index fields; comma-separated or repeated.
        #[arg(long, value_enum, value_delimiter = ',')]
        fields: Vec<SearchFieldArg>,
        #[arg(
            long,
            value_enum,
            help = "Boolean term behavior for lexical/fuzzy search (default: all)"
        )]
        operator: Option<SearchOperatorArg>,
        /// Return the best hit per source/section, or every individual match.
        #[arg(long, value_enum)]
        group_by: Option<SearchGroupArg>,
        /// Ignore Unicode case for exact/regex search.
        #[arg(long)]
        ignore_case: bool,
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=2), help = "Fuzzy edit distance (default: 1, maximum: 2)")]
        fuzzy_distance: Option<u8>,
        #[arg(long, value_parser = parse_duration_ms, help = "Index wait deadline (default: 2s, maximum: 10s)")]
        wait_index: Option<u64>,
        /// Maximum UTF-8 bytes in each discovery excerpt.
        #[arg(long, default_value_t = 256, value_parser = parse_snippet_bytes)]
        snippet_bytes: usize,
    },
    /// List the addressable Items of one module: @id nodes, heading default
    /// names, and resource files, each with its attribute annotations.
    Items {
        /// Exact ModulePath or Vault-relative `.not` path.
        selector: String,
    },
    /// Read authored source by module, path, id, line, or byte range.
    Read {
        /// Exact ModulePath, path, `module/id`, or `path#id` selector.
        selector: String,
        #[arg(long)]
        from_line: Option<usize>,
        #[arg(long, requires = "from_line")]
        lines: Option<usize>,
        #[arg(long, conflicts_with_all = ["from_line", "lines"], value_parser = parse_byte_range)]
        byte_range: Option<notist_service::ByteRange>,
    },
    /// Find references to a logical module.
    References {
        /// Exact ModulePath or `module/id` selector.
        selector: String,
        #[arg(long)]
        include_definition: bool,
        #[arg(long, value_enum, default_value_t)]
        direction: ReferenceDirectionArg,
        /// Maximum UTF-8 bytes in each reference excerpt.
        #[arg(long, default_value_t = 256, value_parser = parse_snippet_bytes)]
        snippet_bytes: usize,
    },
    /// Find the definition at a source byte offset.
    Definition {
        path: PathBuf,
        offset: usize,
        #[arg(long)]
        expected_fingerprint: Option<String>,
    },
    /// Resolve a host coordinate (file path plus line/byte position) into
    /// notist identity: the containing module and its addressable scopes.
    Locate {
        /// Vault-relative (or absolute) file path.
        path: PathBuf,
        /// 1-based line to resolve.
        #[arg(long)]
        line: Option<usize>,
        /// Byte offset to resolve.
        #[arg(long, conflicts_with_all = ["line", "byte_range"])]
        offset: Option<usize>,
        /// Byte range to resolve (uses its start).
        #[arg(long, value_parser = parse_byte_range, conflicts_with_all = ["line", "offset"])]
        byte_range: Option<notist_service::ByteRange>,
    },
    /// Project the attribute environment of an arbitrary region: cut the
    /// range at governing annotation boundaries, merge adjacent pieces with
    /// equal effective attributes, and report the common environment plus
    /// each uniform segment.
    Info {
        /// Module selector (ModulePath, optionally with /ItemName).
        selector: String,
        /// 1-based inclusive line range START..END.
        #[arg(long, value_parser = parse_line_range, conflicts_with_all = ["offset", "byte_range"])]
        line: Option<notist_service::LineRange>,
        /// Byte offset (zero-width point).
        #[arg(long, conflicts_with_all = ["line", "byte_range"])]
        offset: Option<usize>,
        /// Byte range START..END (UTF-8, half-open).
        #[arg(long, value_parser = parse_byte_range, conflicts_with_all = ["line", "offset"])]
        byte_range: Option<notist_service::ByteRange>,
        /// Omit the per-segment source lines.
        #[arg(long)]
        no_content: bool,
        /// Show each entry's declaring annotation provenance (the layered
        /// ancestor view) instead of the merged effective Dict.
        #[arg(long)]
        origins: bool,
    },
    /// Print the ancestor chain of a region with its attribute annotations,
    /// innermost first, ending at the module root.
    Ancestors {
        /// Exact ModulePath, path, `module/id`, or `path#id` selector.
        selector: String,
        /// Select one byte offset instead of the whole selector target.
        #[arg(long, conflicts_with = "byte_range")]
        offset: Option<usize>,
        /// Select a START..END byte region instead of the whole selector target.
        #[arg(long, conflicts_with = "offset", value_parser = parse_byte_range)]
        byte_range: Option<notist_service::ByteRange>,
    },
}

#[derive(Debug, Subcommand)]
enum DaemonAction {
    /// Stop the daemon currently serving a vault.
    Stop,
}

#[derive(Debug, Subcommand)]
enum SkillCommand {
    /// Initialize the official Notist Skill as <SKILLS_ROOT>/notist/SKILL.md.
    Init {
        /// Skills root directory that receives the fixed `notist` skill directory.
        skills_root: PathBuf,
        /// Replace an existing SKILL.md, preserving other files in the directory.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum ModuleKindArg {
    #[default]
    Any,
    Source,
    Virtual,
}

impl From<ModuleKindArg> for notist_service::ModuleKind {
    fn from(value: ModuleKindArg) -> Self {
        match value {
            ModuleKindArg::Any => Self::Any,
            ModuleKindArg::Source => Self::Source,
            ModuleKindArg::Virtual => Self::Virtual,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum SearchModeArg {
    #[default]
    Lexical,
    Exact,
    Fuzzy,
    Regex,
}

impl From<SearchModeArg> for notist_service::SearchMode {
    fn from(value: SearchModeArg) -> Self {
        match value {
            SearchModeArg::Lexical => Self::Lexical,
            SearchModeArg::Exact => Self::Exact,
            SearchModeArg::Fuzzy => Self::Fuzzy,
            SearchModeArg::Regex => Self::Regex,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum SearchOperatorArg {
    #[default]
    All,
    Any,
}

impl From<SearchOperatorArg> for notist_service::SearchOperator {
    fn from(value: SearchOperatorArg) -> Self {
        match value {
            SearchOperatorArg::All => Self::All,
            SearchOperatorArg::Any => Self::Any,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SearchGroupArg {
    Source,
    Section,
    Match,
}

impl From<SearchGroupArg> for notist_service::SearchGroup {
    fn from(value: SearchGroupArg) -> Self {
        match value {
            SearchGroupArg::Source => Self::Source,
            SearchGroupArg::Section => Self::Section,
            SearchGroupArg::Match => Self::Match,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SearchFieldArg {
    Title,
    Heading,
    Id,
    Module,
    Path,
    Tag,
    Body,
    Raw,
    Comment,
}

impl From<SearchFieldArg> for notist_service::SearchField {
    fn from(value: SearchFieldArg) -> Self {
        match value {
            SearchFieldArg::Title => Self::Title,
            SearchFieldArg::Heading => Self::Heading,
            SearchFieldArg::Id => Self::Id,
            SearchFieldArg::Module => Self::Module,
            SearchFieldArg::Path => Self::Path,
            SearchFieldArg::Tag => Self::Tag,
            SearchFieldArg::Body => Self::Body,
            SearchFieldArg::Raw => Self::Raw,
            SearchFieldArg::Comment => Self::Comment,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum ReferenceDirectionArg {
    #[default]
    Incoming,
    Outgoing,
    Both,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum DiagnosticSeverityArg {
    #[default]
    Error,
    Warning,
    Info,
}

impl From<DiagnosticSeverityArg> for notist_service::DiagnosticSeverity {
    fn from(value: DiagnosticSeverityArg) -> Self {
        match value {
            DiagnosticSeverityArg::Error => Self::Error,
            DiagnosticSeverityArg::Warning => Self::Warning,
            DiagnosticSeverityArg::Info => Self::Info,
        }
    }
}

impl From<ReferenceDirectionArg> for notist_service::ReferenceDirection {
    fn from(value: ReferenceDirectionArg) -> Self {
        match value {
            ReferenceDirectionArg::Incoming => Self::Incoming,
            ReferenceDirectionArg::Outgoing => Self::Outgoing,
            ReferenceDirectionArg::Both => Self::Both,
        }
    }
}

#[derive(Debug, Subcommand)]
enum IndexCommand {
    /// Show the index generation and health.
    Status,
    /// Rebuild the current snapshot's derived index.
    Rebuild {
        #[arg(long)]
        wait: bool,
    },
}

fn parse_byte_range(value: &str) -> Result<notist_service::ByteRange, String> {
    let (start, end) = value.split_once("..").ok_or("expected START..END")?;
    let start = start.parse().map_err(|_| "invalid byte-range start")?;
    let end = end.parse().map_err(|_| "invalid byte-range end")?;
    if start > end {
        return Err("byte-range start exceeds end".into());
    }
    Ok(notist_service::ByteRange { start, end })
}

fn parse_line_range(value: &str) -> Result<notist_service::LineRange, String> {
    let (start, end) = value.split_once("..").ok_or("expected START..END")?;
    let start = start.parse().map_err(|_| "invalid line-range start")?;
    let end = end.parse().map_err(|_| "invalid line-range end")?;
    if start == 0 || start > end {
        return Err("line range must be 1-based with START <= END".into());
    }
    Ok(notist_service::LineRange { start, end })
}

fn parse_duration_ms(value: &str) -> Result<u64, String> {
    let millis = if let Some(value) = value.strip_suffix("ms") {
        value.parse().map_err(|_| "invalid millisecond duration")?
    } else if let Some(value) = value.strip_suffix('s') {
        value
            .parse::<u64>()
            .map_err(|_| "invalid second duration")?
            .saturating_mul(1000)
    } else {
        return Err("duration must end in ms or s".into());
    };
    if millis > 10_000 {
        return Err("duration may not exceed 10s".into());
    }
    Ok(millis)
}

fn parse_snippet_bytes(value: &str) -> Result<usize, String> {
    let bytes = value
        .parse::<usize>()
        .map_err(|_| "snippet bytes must be an integer")?;
    if !(64..=2048).contains(&bytes) {
        return Err("snippet bytes must be between 64 and 2048".into());
    }
    Ok(bytes)
}

fn main() -> ExitCode {
    logging::init_from_env();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let exit = error.exit_code().clamp(0, 255) as u8;
            let _ = error.print();
            return ExitCode::from(exit);
        }
    };
    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            let (error_code, exit) = classify_error(error.as_ref());
            eprintln!("error[{error_code}]: {error}");
            ExitCode::from(exit)
        }
    }
}

#[derive(Debug)]
struct UsageError(String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

fn classify_error(error: &(dyn std::error::Error + 'static)) -> (&'static str, u8) {
    let mut current = Some(error);
    while let Some(candidate) = current {
        if candidate.downcast_ref::<UsageError>().is_some() {
            return ("invalid_argument", 2);
        }
        if let Some(error) = candidate.downcast_ref::<std::io::Error>() {
            return match error.kind() {
                std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::NotConnected
                | std::io::ErrorKind::TimedOut => ("service_unavailable", 4),
                std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotFound => {
                    ("invalid_argument", 3)
                }
                _ => ("internal", 70),
            };
        }
        current = candidate.source();
    }
    ("internal", 70)
}

fn run(cli: Cli) -> Result<ExitCode, Box<dyn std::error::Error>> {
    official_docs::ensure_synced()?;
    if !matches!(cli.command, Command::Skill { .. }) {
        for notice in skill::startup_notices() {
            eprintln!("{notice}");
        }
    }
    match cli.command {
        Command::Inspect { command } => {
            run_inspect(command, cli.vault.clone(), cli.no_daemon, cli.color)
        }
        Command::Check {
            scope,
            summary,
            severity,
        } => {
            let root = resolve_vault_root(&cli.vault)?;
            let mut client =
                service::LocalNotistClient::connect(cli.no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::DiagnosticsPage {
                view_id,
                query: notist_service::DiagnosticsQuery {
                    scope,
                    summary_only: summary,
                    severity: severity.into(),
                },
            })?;
            let CoreResponse::DiagnosticsPage(result) = reply.response else {
                return query_response_error("check", reply.response);
            };
            let ok = result.summary.error_count == 0;
            emit_diagnostic_page(&result, cli.color);
            if ok {
                println!("checked {} sources", result.summary.checked_sources);
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::FAILURE)
            }
        }
        Command::Index { command } => {
            let (rebuild, wait) = match command {
                IndexCommand::Status => (false, false),
                IndexCommand::Rebuild { wait } => (true, wait),
            };
            let (_, client, view_id) = connect_cli(cli.vault.clone(), cli.no_daemon)?;
            let effective_wait = rebuild && (wait || cli.no_daemon);
            let reply = client.request(if rebuild {
                CoreRequest::IndexRebuild {
                    view_id,
                    wait: false,
                }
            } else {
                CoreRequest::IndexStatus { view_id }
            })?;
            let CoreResponse::IndexStatus(mut status) = reply.response else {
                return query_response_error("index", reply.response);
            };
            let submitted_operation = status.operation_handle.clone();
            if effective_wait && status.health == "building" {
                if let Some(operation) = &status.operation_handle {
                    eprintln!("waiting for {operation}");
                }
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    let polled = client.request(CoreRequest::IndexStatus { view_id })?;
                    let CoreResponse::IndexStatus(next) = polled.response else {
                        return query_response_error("index", polled.response);
                    };
                    status = next;
                    if status.health != "building" {
                        break;
                    }
                }
                status.operation_handle = submitted_operation;
            }
            if status.health == "error" {
                let error = notist_service::ToolError::new(
                    "index_not_ready",
                    status
                        .message
                        .clone()
                        .unwrap_or_else(|| "index build failed".into()),
                )
                .retryable("run `notist index rebuild --wait` after correcting the index error");
                return query_response_error("index", CoreResponse::QueryError(error));
            }
            println!("Index  {}", status.health);
            println!("Units  {}", status.unit_count);
            if let Some(stamp) = status.stamp {
                println!(
                    "Stamp  {} / {} / {}",
                    stamp.schema_version, stamp.tokenizer_version, stamp.ranking_version
                );
            }
            if let Some(operation) = status.operation_handle {
                println!("Task   {operation}");
            }
            if let Some(message) = status.message {
                println!("Note   {message}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Daemon {
            action: Some(DaemonAction::Stop),
            ..
        } => {
            service::stop_daemon(resolve_vault_root(&cli.vault)?)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Daemon {
            background_child, ..
        } => service::run_daemon(resolve_vault_root(&cli.vault)?, background_child),
        Command::Lsp => lsp::run(cli.no_daemon),
        Command::Skill {
            command: SkillCommand::Init { skills_root, force },
        } => {
            let output = skill::init(skills_root, force)?;
            println!("initialized Notist Skill at {}", output.display());
            Ok(ExitCode::SUCCESS)
        }
        Command::Build { output, clean } => build::run(
            resolve_vault_root(&cli.vault)?,
            output,
            cli.color,
            cli.no_daemon,
            clean,
        ),
        Command::Preview { host, port, open } => preview::run(
            resolve_vault_root(&cli.vault)?,
            host,
            port,
            open,
            cli.color,
            cli.no_daemon,
        ),
    }
}

fn run_inspect(
    command: InspectCommand,
    vault: PathBuf,
    no_daemon: bool,
    color: clap::ColorChoice,
) -> Result<ExitCode, Box<dyn std::error::Error>> {
    match command {
        InspectCommand::Status => {
            let (root, client, view_id) = connect_cli(vault.clone(), no_daemon)?;
            let reply = client.request(CoreRequest::Status { view_id })?;
            let CoreResponse::Status(status) = reply.response else {
                return query_response_error("status", reply.response);
            };
            println!("Vault        {}", status.snapshot.vault.fingerprint);
            println!(
                "Snapshot     {} revision {}",
                status.view_kind, status.snapshot.revision
            );
            println!("Runtime      {}", status.runtime_mode);
            println!("Sources      {}", status.source_count);
            println!("Modules      {}", status.module_count);
            println!("Diagnostics  {}", status.diagnostic_count);
            println!(
                "Index        {} ({} units)",
                status.index.health, status.index.unit_count
            );
            let generation = crate::official_docs::generation_for_root(&root)
                .ok()
                .flatten();
            if let Ok(pid_path) =
                notist_service::transport::daemon_pid_path(&root, generation.as_deref())
            {
                match std::fs::read_to_string(pid_path) {
                    Ok(pid) => println!("Daemon       pid {}", pid.trim()),
                    Err(_) => println!("Daemon       not running"),
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        InspectCommand::Ls {
            target,
            recursive,
            kind,
        } => {
            let (_, client, view_id) = connect_cli(vault.clone(), no_daemon)?;
            let reply = client.request(CoreRequest::ListModules {
                view_id,
                query: notist_service::ModulesQuery {
                    target,
                    recursive,
                    kind: kind.into(),
                },
            })?;
            let CoreResponse::Modules(page) = reply.response else {
                return query_response_error("ls", reply.response);
            };
            println!("{}", plural(page.records.len(), "module"));
            for item in &page.records {
                // Identity-only rows: the `.not` path is not shown here — it
                // is handed over by read's citation footer when editing.
                let suffix = if item.relative_path.is_none() {
                    " — <virtual>".to_owned()
                } else {
                    item.title
                        .as_ref()
                        .map_or(String::new(), |title| format!(" — {title}"))
                };
                println!("{}{}", item.module, suffix);
            }
            Ok(ExitCode::SUCCESS)
        }
        InspectCommand::Search {
            query,
            mode,
            exact,
            fuzzy,
            regex,
            scopes,
            fields,
            operator,
            group_by,
            ignore_case,
            fuzzy_distance,
            wait_index,
            snippet_bytes,
        } => {
            let mode = if exact {
                notist_service::SearchMode::Exact
            } else if fuzzy {
                notist_service::SearchMode::Fuzzy
            } else if regex {
                notist_service::SearchMode::Regex
            } else {
                mode.into()
            };
            let incompatible = match mode {
                notist_service::SearchMode::Exact | notist_service::SearchMode::Regex => {
                    operator.is_some() || fuzzy_distance.is_some() || wait_index.is_some()
                }
                notist_service::SearchMode::Lexical => fuzzy_distance.is_some() || ignore_case,
                notist_service::SearchMode::Fuzzy => ignore_case,
            };
            if incompatible {
                return Err(UsageError(format!(
                    "one or more search options are not valid in {mode:?} mode"
                ))
                .into());
            }
            let root = resolve_vault_root(&vault)?;
            let mut client =
                service::LocalNotistClient::connect(no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::SearchPage {
                view_id,
                query: notist_service::SearchQuery {
                    query: query.clone(),
                    mode,
                    scopes,
                    fields: if fields.is_empty() {
                        notist_service::SearchField::defaults()
                    } else {
                        fields.into_iter().map(Into::into).collect()
                    },
                    operator: operator.unwrap_or_default().into(),
                    group_by: group_by.map(Into::into),
                    ignore_case,
                    fuzzy_distance: fuzzy_distance.unwrap_or(1),
                    wait_index_ms: wait_index.unwrap_or(2000),
                    snippet_bytes,
                },
            })?;
            let CoreResponse::SearchPage(results) = reply.response else {
                return query_response_error("search", reply.response);
            };
            let group = results
                .search
                .as_ref()
                .map(|metadata| match metadata.group_by {
                    notist_service::SearchGroup::Source => "source",
                    notist_service::SearchGroup::Section => "section",
                    notist_service::SearchGroup::Match => "match",
                })
                .unwrap_or("match");
            println!("{}", plural(results.records.len(), group));
            for (index, result) in results.records.iter().enumerate() {
                let score = result.score.map_or(String::new(), |score| {
                    format!(" score={:.3}", score as f64 / 1_000_000.0)
                });
                let position = result.location.line_range.map_or_else(
                    || {
                        format!(
                            "{}@{}..{}",
                            result.location.relative_path.display(),
                            result.location.byte_range.start,
                            result.location.byte_range.end
                        )
                    },
                    |range| {
                        format!(
                            "{}:{}",
                            result.location.relative_path.display(),
                            range.start
                        )
                    },
                );
                println!(
                    "
{}. {} {} field={}{}",
                    index + 1,
                    result.location.module,
                    position,
                    result.matched_field,
                    score
                );
                println!("   {}", result.excerpt.replace('\n', " "));
            }
            for hint in &results.hints {
                println!("hint: {hint}");
            }
            Ok(ExitCode::SUCCESS)
        }
        InspectCommand::Items { selector } => {
            let root = resolve_vault_root(&vault)?;
            let mut client =
                service::LocalNotistClient::connect(no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::Items {
                view_id,
                query: notist_service::ItemsQuery {
                    selector: notist_service::Selector::parse(&selector),
                },
            })?;
            let CoreResponse::Items(items) = reply.response else {
                return query_response_error("items", reply.response);
            };
            println!("{}", plural(items.records.len(), "item"));
            for item in &items.records {
                // Identity commands carry no file paths; scope Items expose
                // their line range so the host Read can fetch the section.
                let position = item.location.line_range.map_or_else(
                    || item.location.relative_path.display().to_string(),
                    |range| format!("lines {}..{}", range.start, range.end),
                );
                let annotations = item
                    .attributes
                    .iter()
                    .map(|annotation| format_annotation(annotation, "@(", ")"))
                    .collect::<Vec<_>>()
                    .join(" ");
                // `@id`-named Items are spelled differently from heading
                // defaults: the name is an author-granted identity, not text.
                let kind = if item.origin == "id" {
                    format!("{}@id", item.kind)
                } else {
                    item.kind.clone()
                };
                let line = format!(
                    "{} {}{} {} {}{}",
                    item.name,
                    kind,
                    item.level
                        .map_or(String::new(), |level| format!(" L{level}")),
                    position,
                    annotations,
                    if item.ambiguous { " (ambiguous)" } else { "" },
                );
                println!("{}", line.trim_end());
            }
            Ok(ExitCode::SUCCESS)
        }
        InspectCommand::Read {
            selector,
            from_line,
            lines,
            byte_range,
        } => {
            let (_, client, view_id) = connect_cli(vault.clone(), no_daemon)?;
            let reply = client.request(CoreRequest::ReadSource {
                view_id,
                query: notist_service::ReadQuery {
                    selector: notist_service::Selector::parse(&selector),
                    window: notist_service::ReadWindow {
                        from_line,
                        lines,
                        byte_range,
                    },
                },
            })?;
            let CoreResponse::SourcePage(result) = reply.response else {
                return query_response_error("read", reply.response);
            };
            if let Some(chunk) = result.records.first() {
                let palette = Palette::stdout(color);
                let start = chunk.location.line_range.map_or(1, |range| range.start);
                for (offset, line) in chunk.source.lines().enumerate() {
                    println!(
                        "{} {} {}",
                        palette.dim(&format!("{:>5}", start + offset)),
                        palette.dim("|"),
                        line
                    );
                }
                // Citation footer: the path handoff to the host editor.
                let location = &chunk.location;
                let mut footer = format!(
                    "-- {} {}",
                    location.module,
                    location.relative_path.display()
                );
                if let Some(range) = location.line_range {
                    footer.push_str(&format!(" lines {}..{}", range.start, range.end));
                }
                footer.push_str(&format!(
                    " bytes {}..{}",
                    location.byte_range.start, location.byte_range.end
                ));
                footer.push_str(&format!(" fingerprint {}", location.source_fingerprint));
                println!("{footer}");
            }
            Ok(ExitCode::SUCCESS)
        }
        InspectCommand::References {
            selector,
            include_definition,
            direction,
            snippet_bytes,
        } => {
            let direction_value = notist_service::ReferenceDirection::from(direction);
            let direction_label = match direction_value {
                notist_service::ReferenceDirection::Incoming => "incoming",
                notist_service::ReferenceDirection::Outgoing => "outgoing",
                notist_service::ReferenceDirection::Both => "both",
            };
            let root = resolve_vault_root(&vault)?;
            let mut client =
                service::LocalNotistClient::connect(no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::ReferencesPage {
                view_id,
                query: notist_service::ReferencesQuery {
                    selector: notist_service::Selector::parse(&selector),
                    direction: direction_value,
                    include_definition,
                    snippet_bytes,
                },
            })?;
            let CoreResponse::ReferencesPage(locations) = reply.response else {
                return query_response_error("references", reply.response);
            };
            println!(
                "{} ({})",
                plural(locations.records.len(), "reference"),
                direction_label
            );
            for item in &locations.records {
                let position = item.location.line_range.map_or_else(
                    || {
                        format!(
                            "{}@{}..{}",
                            item.location.relative_path.display(),
                            item.location.byte_range.start,
                            item.location.byte_range.end
                        )
                    },
                    |range| format!("{}:{}", item.location.relative_path.display(), range.start),
                );
                let definition_mark = if item.is_definition {
                    " (definition)"
                } else {
                    ""
                };
                println!("{} -> {} {}{}", item.source, item.target, position, definition_mark);
                if !item.excerpt.is_empty() {
                    let ellipsis = if item.excerpt_truncated { " …" } else { "" };
                    println!("    {}{}", item.excerpt.replace('\n', " "), ellipsis);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        InspectCommand::Definition {
            path,
            offset,
            expected_fingerprint,
        } => {
            let root = resolve_vault_root(&vault)?;
            let path = dunce::canonicalize(if path.is_absolute() {
                path
            } else {
                vault.join(path)
            })?;
            let mut client =
                service::LocalNotistClient::connect(no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::DefinitionLocation {
                view_id,
                query: notist_service::DefinitionQuery {
                    path: path.clone(),
                    offset,
                    expected_fingerprint,
                },
            })?;
            let CoreResponse::DefinitionLocation(definition) = reply.response else {
                return query_response_error("inspect definition", reply.response);
            };
            if let Some(definition) = definition {
                let identity = match &definition.id {
                    Some(name) => format!("{}/{}", definition.module, name),
                    None => definition.module.clone(),
                };
                println!(
                    "{} {}:{}..{}",
                    identity,
                    definition.relative_path.display(),
                    definition.byte_range.start,
                    definition.byte_range.end
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        InspectCommand::Locate {
            path,
            line,
            offset,
            byte_range,
        } => {
            let (_, client, view_id) = connect_cli(vault.clone(), no_daemon)?;
            let reply = client.request(CoreRequest::Locate {
                view_id,
                query: notist_service::LocateQuery {
                    path,
                    line,
                    offset,
                    byte_range,
                },
            })?;
            let CoreResponse::Locate(record) = reply.response else {
                return query_response_error("locate", reply.response);
            };
            let mut parts = vec![record.module.clone()];
            parts.extend(record.breadcrumb.clone());
            let head = match &record.item {
                Some(item) => format!("{} --item {item}", parts.join(" ")),
                None => parts.join(" "),
            };
            let position = record.point_line.map_or_else(
                || record.relative_path.display().to_string(),
                |point| format!("{}:{}", record.relative_path.display(), point),
            );
            println!("{head} {position}");
            Ok(ExitCode::SUCCESS)
        }
        InspectCommand::Info {
            selector,
            line,
            offset,
            byte_range,
            no_content,
            origins,
        } => {
            let (_, client, view_id) = connect_cli(vault.clone(), no_daemon)?;
            let reply = client.request(CoreRequest::Region {
                view_id,
                query: notist_service::RegionQuery {
                    selector: notist_service::Selector::parse(&selector),
                    offset,
                    byte_range,
                    line_range: line,
                    include_content: !no_content,
                },
            })?;
            let CoreResponse::Region(page) = reply.response else {
                return query_response_error("inspect info", reply.response);
            };
            let palette = Palette::stdout(color);
            for record in &page.records {
                print_region_record(record, &palette, origins);
            }
            Ok(ExitCode::SUCCESS)
        }
        InspectCommand::Ancestors {
            selector,
            offset,
            byte_range,
        } => {
            let (_, client, view_id) = connect_cli(vault.clone(), no_daemon)?;
            let reply = client.request(CoreRequest::Ancestors {
                view_id,
                query: notist_service::AncestorsQuery {
                    selector: notist_service::Selector::parse(&selector),
                    offset,
                    byte_range,
                },
            })?;
            let CoreResponse::Ancestors(ancestors) = reply.response else {
                return query_response_error("ancestors", reply.response);
            };
            for root in &ancestors.records {
                println!(
                    "module {} fingerprint {}",
                    root.location.module, root.location.source_fingerprint,
                );
                print_ancestor_tree(root, 0);
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// ANSI styling for human output, governed by `--color` and stdout TTY
/// detection; styling never changes the logical result, only its rendering.
struct Palette {
    enabled: bool,
}

impl Palette {
    fn stdout(color: clap::ColorChoice) -> Self {
        Self {
            enabled: matches!(color, clap::ColorChoice::Always)
                || matches!(color, clap::ColorChoice::Auto) && std::io::stdout().is_terminal(),
        }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.enabled && !text.is_empty() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    fn bold(&self, text: &str) -> String {
        self.paint("1", text)
    }

    fn dim(&self, text: &str) -> String {
        self.paint("2", text)
    }

    /// Content bodies at the regular foreground tone: in `info` the source
    /// lines are context, the structure is the signal — they read as text,
    /// one step above the dimmed metadata.
    fn body(&self, text: &str) -> String {
        self.paint("37", text)
    }

    fn green(&self, text: &str) -> String {
        self.paint("32", text)
    }

    fn cyan(&self, text: &str) -> String {
        self.paint("36", text)
    }
}

/// Prints one ancestor subtree in document order, root first, indenting each
/// level; each line reads like the annotation surface it came from. Rows
/// carry line ranges only — identity commands show no file paths.
fn print_ancestor_tree(record: &notist_service::AncestorRecord, depth: usize) {
    let position = record.location.line_range.map_or_else(
        || {
            format!(
                "bytes {}..{}",
                record.location.byte_range.start, record.location.byte_range.end
            )
        },
        |range| format!("lines {}..{}", range.start, range.end),
    );
    let annotations = record
        .attributes
        .iter()
        .map(|annotation| {
            if record.kind == "module" {
                format_annotation(annotation, "@!(", ")")
            } else {
                format_annotation(annotation, "@(", ")")
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let line = format!(
        "{}{}{}{} {} {}",
        "  ".repeat(depth),
        record.kind,
        record
            .name
            .as_ref()
            .map_or(String::new(), |name| format!(" {name:?}")),
        record
            .level
            .map_or(String::new(), |level| format!(" L{level}")),
        position,
        annotations,
    );
    println!("{}", line.trim_end());
    for child in &record.children {
        print_ancestor_tree(child, depth + 1);
    }
}

/// Prints the region attribute projection: header with the module identity
/// and fingerprint, the common environment, then one block per uniform
/// segment. The ModulePath is the file spelling, so no separate path echo.
/// Color roles are token-based and uniform everywhere: bold = structural
/// labels, cyan = every identity (module path, Item and node names, also
/// inside origins), green = attribute keys,
/// dim = all metadata (paths, fingerprints, every line/byte range, node
/// kinds, levels, the gutter), body = segment source bodies.
fn print_region_record(
    record: &notist_service::RegionRecord,
    palette: &Palette,
    origins: bool,
) {
    let header = [
        palette.cyan(&format!("<{}>", record.module)),
        palette.dim(&format!(
            "lines {}..{}",
            record.line_range.start, record.line_range.end
        )),
        palette.dim(&format!(
            "bytes {}..{}",
            record.byte_range.start, record.byte_range.end
        )),
        palette.dim(&format!("fingerprint {}", record.source_fingerprint)),
    ]
    .join(" ");
    println!("{} {}", palette.bold("module"), header);
    if let Some(container) = &record.container {
        print_region_container("container", container, palette);
    }
    if origins && !record.common.is_empty() {
        println!("{} {}", palette.bold("common"), record.common.len());
        for group in &record.common {
            print_region_entry(group, palette);
        }
    }
    println!("{} {}", palette.bold("segments"), record.segments.len());
    for (index, segment) in record.segments.iter().enumerate() {
        // Same grammar as `module` and `container`: identity first, then the
        // coordinate pair. The identity is the innermost containing Item.
        let mut head = Vec::new();
        if let Some(name) = &segment.item {
            head.push(palette.cyan(&format!("<{name}>")));
        }
        head.push(palette.dim(&format!(
            "lines {}..{}",
            segment.line_range.start, segment.line_range.end
        )));
        head.push(palette.dim(&format!(
            "bytes {}..{}",
            segment.byte_range.start, segment.byte_range.end
        )));
        println!(
            "{} {}",
            palette.bold(&format!("[{}]", index + 1)),
            head.join(" ")
        );
        if origins {
            for group in &segment.attributes {
                print_region_entry(group, palette);
            }
        } else {
            // The merged effective environment as a notist Dict literal:
            // every key governing this segment, sorted.
            let mut entries: Vec<(String, String)> = segment
                .attributes
                .iter()
                .flat_map(|group| {
                    group
                        .entries
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect::<Vec<_>>()
                })
                .chain(
                    record
                        .common
                        .iter()
                        .flat_map(|group| {
                            group
                                .entries
                                .iter()
                                .filter(|(key, _)| {
                                    !segment.attributes.iter().any(|group| {
                                        group.entries.iter().any(|(k, _)| k.as_str() == *key)
                                    })
                                })
                                .map(|(key, value)| (key.clone(), value.clone()))
                                .collect::<Vec<_>>()
                        }),
                )
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            entries.dedup_by(|a, b| a.0 == b.0);
            println!("    {}", dict_literal(&entries));
        }
        for line in &segment.content {
            println!(
                "{} {} {}",
                palette.dim(&format!("{:>5}", line.number)),
                palette.dim("|"),
                palette.body(&line.text),
            );
        }
    }
}

/// Formats entries as a notist Dict literal: `(k: "v", ...)`, `(:)` empty.
/// Identifier keys stay bare; string-ish values are quoted so the literal
/// is valid Dict syntax.
fn dict_literal(entries: &[(String, String)]) -> String {
    if entries.is_empty() {
        // notist: `()` is Unit; the empty Dict literal is `(:)`.
        return "(:)".to_owned();
    }
    let inner = entries
        .iter()
        .map(|(key, value)| format!("{}: {}", dict_key(key), dict_value(value)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("({inner})")
}

fn dict_key(key: &str) -> String {
    let identifier = !key.is_empty()
        && key
            .chars()
            .enumerate()
            .all(|(index, c)| {
                c == '_' || c.is_alphanumeric() && !c.is_numeric() || (index > 0 && c.is_numeric())
            });
    if identifier {
        key.to_owned()
    } else {
        let escaped = key.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    }
}

fn dict_value(value: &str) -> String {
    // Values are canonical strings; spell scalar-looking values bare so
    // `true` / `42` round-trip as their literal shapes.
    if value == "true"
        || value == "false"
        || value.parse::<i64>().is_ok()
        || value.parse::<f64>().is_ok()
    {
        value.to_owned()
    } else {
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    }
}

fn print_region_container(
    label: &str,
    container: &notist_service::RegionContainer,
    palette: &Palette,
) {
    let identity = container
        .path
        .as_deref()
        .map_or_else(|| "<anonymous>".to_owned(), |path| format!("<{path}>"));
    println!(
        "{} {} lines {}..{} bytes {}..{}",
        palette.bold(label),
        palette.cyan(&identity),
        container.line_range.start,
        container.line_range.end,
        container.byte_range.start,
        container.byte_range.end,
    );
}

/// Prints one declaring annotation and the keys it governs, spelled the way
/// it was declared: `@(k1: v1, k2: v2)  <- <ItemPath>  lines a..b`.
fn print_region_entry(group: &notist_service::RegionEntryGroup, palette: &Palette) {
    let pairs = group
        .entries
        .iter()
        .map(|(key, value)| format!("{}: {}", palette.green(key), value))
        .collect::<Vec<_>>()
        .join(", ");
    let origins = group
        .origins
        .iter()
        .map(|origin| {
            let identity = if origin.kind == "module" {
                palette.dim("module")
            } else {
                palette.cyan(
                    &origin
                        .path
                        .as_deref()
                        .map_or_else(|| "<anonymous>".to_owned(), |path| format!("<{path}>")),
                )
            };
            palette.dim(&format!(
                "{} lines {}..{} bytes {}..{}",
                identity,
                origin.line_range.start,
                origin.line_range.end,
                origin.byte_range.start,
                origin.byte_range.end
            ))
        })
        .collect::<Vec<_>>()
        .join(" | ");
    println!(
        "    {} <- {}",
        palette.cyan(&format!("@({pairs})")),
        origins
    );
}

fn open_disk_view(
    client: &mut service::LocalNotistClient,
    root: PathBuf,
) -> Result<notist_service::ServiceViewId, Box<dyn std::error::Error>> {
    let reply = client.request(CoreRequest::OpenView {
        root,
        kind: ProtocolViewKind::Disk,
    })?;
    let CoreResponse::Opened { view_id, .. } = reply.response else {
        return Err("daemon returned an unexpected open-view response".into());
    };
    Ok(view_id)
}

fn connect_cli(
    root: PathBuf,
    no_daemon: bool,
) -> Result<
    (
        PathBuf,
        service::LocalNotistClient,
        notist_service::ServiceViewId,
    ),
    Box<dyn std::error::Error>,
> {
    let root = resolve_vault_root(&root)?;
    let mut client = service::LocalNotistClient::connect(no_daemon, ClientKind::Cli, root.clone())?;
    let view_id = open_disk_view(&mut client, root.clone())?;
    Ok((root, client, view_id))
}

fn query_response_error(
    command: &str,
    response: CoreResponse,
) -> Result<ExitCode, Box<dyn std::error::Error>> {
    if let CoreResponse::QueryError(error) = response {
        eprintln!("error[{}]: {}", error.code, error.message);
        if let Some(hint) = error.hint {
            eprintln!("hint: {hint}");
        }
        for candidate in error.candidates {
            eprintln!("  {candidate}");
        }
        Ok(ExitCode::from(3))
    } else {
        Err(format!("service returned an unexpected {command} response").into())
    }
}

/// Formats a count header with correct plurality ("1 source", "2 sources").
fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// Renders one attribute set in its annotation surface spelling, so a chain
/// line reads like the source it came from.
fn format_annotation(
    annotation: &notist_service::AttributeRecord,
    open: &str,
    close: &str,
) -> String {
    let mut items = Vec::new();
    if let Some(id) = &annotation.id {
        items.push(format!("id: {id}"));
    }
    for (key, value) in &annotation.properties {
        items.push(format!("{key}: {value}"));
    }
    for tag in &annotation.tags {
        items.push(format!("tags: {tag}"));
    }
    for class in &annotation.classes {
        items.push(format!("class: {class}"));
    }
    if items.is_empty() {
        return String::new();
    }
    format!("{open}{}{close}", items.join(", "))
}

fn emit_diagnostic_page(result: &notist_service::DiagnosticsResult, color: clap::ColorChoice) {
    let color = matches!(color, clap::ColorChoice::Always)
        || matches!(color, clap::ColorChoice::Auto) && std::io::stderr().is_terminal();
    for diagnostic in &result.diagnostics.records {
        if color {
            eprintln!(
                "\x1b[1;31merror[{}]\x1b[0m: {}",
                diagnostic.code, diagnostic.message
            );
        } else {
            eprintln!("error[{}]: {}", diagnostic.code, diagnostic.message);
        }
        if let Some(location) = &diagnostic.location {
            let line = location.line_range.map_or(0, |range| range.start);
            eprintln!("  --> {}:{}", location.relative_path.display(), line);
            if let Some(excerpt) = &diagnostic.excerpt {
                eprintln!("   |");
                let excerpt_start = diagnostic.excerpt_line_start.unwrap_or(line);
                let lines = excerpt.lines().collect::<Vec<_>>();
                let target = line
                    .saturating_sub(excerpt_start)
                    .min(lines.len().saturating_sub(1));
                let frame_start = target.saturating_sub(2);
                let frame_end = (target + 3).min(lines.len());
                for (offset, text) in lines[frame_start..frame_end].iter().enumerate() {
                    let current = excerpt_start + frame_start + offset;
                    eprintln!("{:>3} | {}", current, text);
                    if current == line {
                        let column = diagnostic.excerpt_range.map_or(0, |range| {
                            let relative = location
                                .byte_range
                                .start
                                .saturating_sub(range.start)
                                .min(excerpt.len());
                            excerpt[..relative]
                                .rsplit('\n')
                                .next()
                                .unwrap_or("")
                                .chars()
                                .count()
                        });
                        let width = location
                            .byte_range
                            .end
                            .saturating_sub(location.byte_range.start)
                            .clamp(1, 40);
                        eprintln!("    | {}{}", " ".repeat(column), "^".repeat(width));
                    }
                }
            }
        }
        eprintln!();
    }
    eprintln!(
        "{} diagnostics in {} sources",
        result.summary.total_diagnostics, result.summary.checked_sources
    );
}

pub(crate) fn emit_service_diagnostics(diagnostics: &[notist_service::DiagnosticRecord]) {
    for diagnostic in diagnostics {
        let path = diagnostic
            .path
            .as_ref()
            .map_or_else(|| "<workspace>".into(), |path| path.display().to_string());
        let range = diagnostic.range.map_or_else(String::new, |range| {
            format!(":{}..{}", range.start, range.end)
        });
        eprintln!(
            "{path}{range}: {} [{}]",
            diagnostic.message, diagnostic.code
        );
    }
}
