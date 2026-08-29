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
  inspect:     status, modules, search, outline, read, references, definition
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
    /// Investigate a Vault: status, modules, search, outline, read, references, definition.
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
    /// List modules in the vault.
    Modules {
        #[arg(long)]
        prefix: Option<String>,
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
    /// Print the evaluated heading outline for one module.
    Outline {
        /// Exact ModulePath or Vault-relative `.not` path.
        selector: String,
        #[arg(long, default_value_t = 6, value_parser = clap::value_parser!(u8).range(1..=6))]
        depth: u8,
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
}

#[derive(Debug, Subcommand)]
enum DaemonAction {
    /// Stop the daemon currently serving a vault.
    Stop,
}

#[derive(Debug, Subcommand)]
enum SkillCommand {
    /// Initialize the official Notist Skill in a directory.
    Init {
        output: PathBuf,
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
    match cli.command {
        Command::Inspect { command } => run_inspect(command, cli.vault.clone(), cli.no_daemon),
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
            let (_, mut client, view_id) = connect_cli(cli.vault.clone(), cli.no_daemon)?;
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
            background_child,
            ..
        } => service::run_daemon(resolve_vault_root(&cli.vault)?, background_child),
        Command::Lsp => lsp::run(cli.no_daemon),
        Command::Skill {
            command: SkillCommand::Init { output, force },
        } => {
            let output = skill::init(output, force)?;
            println!("initialized Notist Skill at {}", output.display());
            Ok(ExitCode::SUCCESS)
        }
        Command::Build {
            output,
            clean,
        } => build::run(
            resolve_vault_root(&cli.vault)?,
            output,
            cli.color,
            cli.no_daemon,
            clean,
        ),
        Command::Preview {
            host,
            port,
            open,
        } => preview::run(
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
) -> Result<ExitCode, Box<dyn std::error::Error>> {
    match command {
        InspectCommand::Status => {
            let (root, mut client, view_id) = connect_cli(vault.clone(), no_daemon)?;
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
            let generation =
                crate::official_docs::generation_for_root(&root)
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
        InspectCommand::Modules { prefix, kind } => {
            let (_, mut client, view_id) = connect_cli(vault.clone(), no_daemon)?;
            let reply = client.request(CoreRequest::ListModules {
                view_id,
                query: notist_service::ModulesQuery {
                    prefix,
                    kind: kind.into(),
                },
            })?;
            let CoreResponse::Modules(page) = reply.response else {
                return query_response_error("modules", reply.response);
            };
            for item in &page.records {
                let path = item
                    .relative_path
                    .as_ref()
                    .map_or("<virtual>".into(), |path| path.display().to_string());
                let title = item
                    .title
                    .as_ref()
                    .map_or(String::new(), |title| format!(" — {title}"));
                println!("{}  {}{}", item.module, path, title);
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
                    notist_service::SearchGroup::Source => "sources",
                    notist_service::SearchGroup::Section => "sections",
                    notist_service::SearchGroup::Match => "matches",
                })
                .unwrap_or("matches");
            println!("{} {group}", results.records.len());
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
{}. {}  {} field={}{}",
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
        InspectCommand::Outline { selector, depth } => {
            let root = resolve_vault_root(&vault)?;
            let mut client =
                service::LocalNotistClient::connect(no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::OutlineModule {
                view_id,
                query: notist_service::OutlineQuery {
                    selector: notist_service::Selector::parse(&selector),
                    depth,
                },
            })?;
            let CoreResponse::OutlinePage(outline) = reply.response else {
                return query_response_error("outline", reply.response);
            };
            for symbol in &outline.records {
                println!(
                    "{}{}  {}:{}",
                    "  ".repeat(symbol.level.saturating_sub(1) as usize),
                    symbol.name,
                    symbol.location.relative_path.display(),
                    symbol.location.line_range.map_or(0, |range| range.start)
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        InspectCommand::Read {
            selector,
            from_line,
            lines,
            byte_range,
        } => {
            let (_, mut client, view_id) = connect_cli(vault.clone(), no_daemon)?;
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
                let start = chunk.location.line_range.map_or(1, |range| range.start);
                for (offset, line) in chunk.source.lines().enumerate() {
                    println!("{:>5} | {}", start + offset, line);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        InspectCommand::References {
            selector,
            include_definition,
            direction,
            snippet_bytes,
        } => {
            let root = resolve_vault_root(&vault)?;
            let mut client =
                service::LocalNotistClient::connect(no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::ReferencesPage {
                view_id,
                query: notist_service::ReferencesQuery {
                    selector: notist_service::Selector::parse(&selector),
                    direction: direction.into(),
                    include_definition,
                    snippet_bytes,
                },
            })?;
            let CoreResponse::ReferencesPage(locations) = reply.response else {
                return query_response_error("references", reply.response);
            };
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
                println!("{} -> {}  {}", item.source, item.target, position);
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
                println!(
                    "{}:{}..{}",
                    definition.relative_path.display(),
                    definition.byte_range.start,
                    definition.byte_range.end
                );
            }
            Ok(ExitCode::SUCCESS)
        }
    }
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
        result.summary.total_diagnostics,
        result.summary.checked_sources
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
