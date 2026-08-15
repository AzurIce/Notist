use std::ffi::OsString;
use std::io::IsTerminal;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process::{ExitCode, Stdio};

use clap::{Args, Parser, Subcommand, ValueEnum};
use notist_analysis::resolve_vault_root;
use notist_service::protocol::ClientKind;
use notist_service::{CoreRequest, CoreResponse, ProtocolViewKind};
use sha2::{Digest, Sha256};

mod build;
mod lsp;
mod official_docs;
mod output;
mod preview;
mod resources;
mod service;
mod skill;

use output::OutputFormat;

#[derive(Debug, Parser)]
#[command(name = "notist", version, about, arg_required_else_help = true)]
struct Cli {
    /// Control colored diagnostic output.
    #[arg(long, value_enum, default_value_t = clap::ColorChoice::Auto, global = true)]
    color: clap::ColorChoice,

    /// Run the application service in this process instead of using the local daemon.
    #[arg(long, global = true)]
    no_daemon: bool,

    /// Select human-readable text or versioned JSON output.
    #[arg(long, value_enum, default_value_t, global = true)]
    format: OutputFormat,

    /// Page the current bounded human-readable result.
    #[arg(long, value_enum, default_value_t, global = true)]
    pager: PagerChoice,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the shared local Notist daemon for one vault, or stop the running one.
    Daemon {
        #[command(subcommand)]
        action: Option<DaemonAction>,
        /// Root directory of the vault this daemon serves.
        #[arg(default_value = ".")]
        root: PathBuf,
        #[arg(long, hide = true)]
        background_child: bool,
    },
    /// Run the Notist language server over standard input and output.
    Lsp,
    /// Create resources that teach an Agent how to use Notist.
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Show a compact Vault, snapshot, diagnostics, and index summary.
    Status {
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// List modules with bounded, resumable output.
    Modules {
        #[arg(default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        prefix: Option<String>,
        #[arg(long, value_enum, default_value_t)]
        kind: ModuleKindArg,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Check module paths and references in a Notist workspace.
    Check {
        /// Root directory of the Notist workspace.
        #[arg(default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        summary: bool,
        #[arg(long, value_enum, default_value_t)]
        severity: DiagnosticSeverityArg,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Search captured source context in a vault.
    #[command(
        after_help = "Examples:\n  notist search \"workspace snapshot\" docs\n  notist search --exact \"WorkspaceSnapshot\" docs --group-by match\n  notist search --fuzzy \"WorkspaceSnaphot\" docs\n\nLexical/fuzzy search groups by source by default; exact/regex returns each match.\nAn incomplete page is enough to select a positive candidate, but not to prove absence or completeness."
    )]
    Search {
        /// Natural-language terms, an identifier, or a literal/regex pattern selected by mode.
        query: String,
        #[arg(default_value = ".")]
        root: PathBuf,
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
        #[command(flatten)]
        page: SearchPageArgs,
    },
    /// Print the evaluated heading outline for one module.
    Outline {
        /// Exact ModulePath or Vault-relative `.not` path.
        selector: String,
        #[arg(default_value = ".")]
        root: PathBuf,
        #[arg(long, default_value_t = 6, value_parser = clap::value_parser!(u8).range(1..=6))]
        depth: u8,
        #[command(flatten)]
        page: OutlinePageArgs,
    },
    /// Read bounded authored source by module, path, id, line, or byte range.
    Read {
        /// Exact ModulePath, path, or `module#id` selector.
        selector: String,
        #[arg(default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        from_line: Option<usize>,
        #[arg(long, requires = "from_line")]
        lines: Option<usize>,
        #[arg(long, conflicts_with_all = ["from_line", "lines"], value_parser = parse_byte_range)]
        byte_range: Option<notist_service::ByteRange>,
        #[command(flatten)]
        page: ReadPageArgs,
    },
    /// Find references to a logical module.
    References {
        /// Exact ModulePath or `module#id` selector.
        selector: String,
        #[arg(default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        include_definition: bool,
        #[arg(long, value_enum, default_value_t)]
        direction: ReferenceDirectionArg,
        /// Maximum UTF-8 bytes in each reference excerpt.
        #[arg(long, default_value_t = 256, value_parser = parse_snippet_bytes)]
        snippet_bytes: usize,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Run a protocol-independent semantic query.
    Query {
        #[command(subcommand)]
        query: QueryCommand,
    },
    /// Apply preconditioned source edits through the shared service.
    Edit {
        #[command(subcommand)]
        edit: EditCommand,
    },
    /// Inspect or rebuild the derived lexical search index.
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },
    /// Access bounded implementation-oriented diagnostics.
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
    },
    /// Write complete snapshot artifacts to an explicit file.
    Export {
        #[command(subcommand)]
        command: ExportCommand,
    },
    /// Build a Notist workspace as a multi-page static HTML site.
    Build {
        /// Root directory of the Notist workspace.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Directory to write the generated site.
        #[arg(short, long, default_value = "dist")]
        output: PathBuf,
        /// Remove the selected output directory before writing this build.
        #[arg(long)]
        clean: bool,
    },
    /// Preview a Notist workspace in a local browser with live reload.
    Preview {
        /// Root directory of the Notist workspace.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Network interface on which the preview server listens.
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        /// TCP port. Zero asks the operating system for an available port.
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Open the preview URL in the default browser.
        #[arg(long)]
        open: bool,
    },
}

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Self::Daemon {
                action: Some(DaemonAction::Stop { .. }),
                ..
            } => "daemon stop",
            Self::Daemon { .. } => "daemon",
            Self::Lsp => "lsp",
            Self::Skill { .. } => "skill init",
            Self::Status { .. } => "status",
            Self::Modules { .. } => "modules",
            Self::Check { .. } => "check",
            Self::Search { .. } => "search",
            Self::Outline { .. } => "outline",
            Self::Read { .. } => "read",
            Self::References { .. } => "references",
            Self::Query { .. } => "query definition",
            Self::Edit {
                edit: EditCommand::Replace { .. },
            } => "edit replace",
            Self::Edit {
                edit: EditCommand::Rename { .. },
            } => "edit rename",
            Self::Index { .. } => "index",
            Self::Debug { .. } => "debug inspect",
            Self::Export { .. } => "export",
            Self::Build { .. } => "build",
            Self::Preview { .. } => "preview",
        }
    }
}

#[derive(Debug, Subcommand)]
enum DaemonAction {
    /// Stop the daemon currently serving a vault.
    Stop {
        /// Root directory of the vault whose daemon should stop.
        #[arg(default_value = ".")]
        root: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum QueryCommand {
    /// Find the definition at a source byte offset.
    Definition {
        path: PathBuf,
        offset: usize,
        #[arg(default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        expected_fingerprint: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum EditCommand {
    /// Replace one byte range after proposing and validating an edit plan.
    Replace {
        path: PathBuf,
        start: usize,
        end: usize,
        replacement: String,
        #[arg(long)]
        idempotency_key: String,
        #[arg(long)]
        expected_fingerprint: String,
        #[arg(long)]
        yes: bool,
    },
    /// Rename a source while preserving its stable file identity.
    Rename {
        from: PathBuf,
        to: PathBuf,
        #[arg(long)]
        idempotency_key: String,
        #[arg(long)]
        expected_fingerprint: String,
        #[arg(long)]
        yes: bool,
    },
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

#[derive(Clone, Debug, Default, Args)]
struct PageArgs {
    #[arg(
        short = 'n',
        long,
        help = "Page item limit (default: 20, maximum: 100)"
    )]
    limit: Option<usize>,
    #[arg(
        long,
        help = "Logical result budget in bytes (default: 16384, maximum: 65536)"
    )]
    max_bytes: Option<usize>,
    #[arg(long, help = "Stable continuation cursor (maximum: 4096 bytes)")]
    cursor: Option<String>,
}

#[derive(Clone, Debug, Default, Args)]
struct SearchPageArgs {
    #[arg(short = 'n', long, help = "Page item limit (default: 8, maximum: 100)")]
    limit: Option<usize>,
    #[arg(
        long,
        help = "Logical result budget in bytes (default: 16384, maximum: 65536)"
    )]
    max_bytes: Option<usize>,
    #[arg(long, help = "Stable continuation cursor (maximum: 4096 bytes)")]
    cursor: Option<String>,
}

#[derive(Clone, Debug, Default, Args)]
struct OutlinePageArgs {
    #[arg(
        short = 'n',
        long,
        help = "Page item limit (default: 100, maximum: 100)"
    )]
    limit: Option<usize>,
    #[arg(
        long,
        help = "Logical result budget in bytes (default: 16384, maximum: 65536)"
    )]
    max_bytes: Option<usize>,
    #[arg(long, help = "Stable continuation cursor (maximum: 4096 bytes)")]
    cursor: Option<String>,
}

#[derive(Clone, Debug, Default, Args)]
struct ReadPageArgs {
    #[arg(
        long,
        help = "Logical result budget in bytes (default: 16384, maximum: 65536)"
    )]
    max_bytes: Option<usize>,
    #[arg(long, help = "Stable continuation cursor (maximum: 4096 bytes)")]
    cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
enum PagerChoice {
    #[default]
    Auto,
    Always,
    Never,
}

impl From<PageArgs> for notist_service::PageRequest {
    fn from(value: PageArgs) -> Self {
        Self {
            limit: value.limit,
            max_bytes: value.max_bytes,
            cursor: value.cursor,
        }
    }
}

impl From<SearchPageArgs> for notist_service::PageRequest {
    fn from(value: SearchPageArgs) -> Self {
        Self {
            limit: value.limit,
            max_bytes: value.max_bytes,
            cursor: value.cursor,
        }
    }
}

impl From<OutlinePageArgs> for notist_service::PageRequest {
    fn from(value: OutlinePageArgs) -> Self {
        Self {
            limit: value.limit,
            max_bytes: value.max_bytes,
            cursor: value.cursor,
        }
    }
}

impl From<ReadPageArgs> for notist_service::PageRequest {
    fn from(value: ReadPageArgs) -> Self {
        Self {
            limit: None,
            max_bytes: value.max_bytes,
            cursor: value.cursor,
        }
    }
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
    Status {
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// Rebuild the current snapshot's derived index.
    Rebuild {
        #[arg(default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        wait: bool,
    },
}

#[derive(Debug, Subcommand)]
enum DebugCommand {
    /// Inspect one bounded internal projection.
    Inspect {
        #[arg(default_value = ".")]
        root: PathBuf,
        #[arg(long, value_enum, default_value_t)]
        section: DebugSection,
        #[arg(long)]
        module: Option<String>,
        #[command(flatten)]
        page: PageArgs,
    },
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum DebugSection {
    #[default]
    Modules,
    References,
    Semantic,
}

#[derive(Debug, Subcommand)]
enum ExportCommand {
    /// Export the complete diagnostic artifact.
    Diagnostics {
        #[arg(default_value = ".")]
        root: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long = "export-format", value_enum, default_value_t)]
        export_format: ExportFormatArg,
    },
    /// Export the complete snapshot projection.
    Snapshot {
        #[arg(default_value = ".")]
        root: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long = "export-format", value_enum, default_value_t)]
        export_format: ExportFormatArg,
    },
    /// Export the complete Vault outline.
    Outline {
        #[arg(default_value = ".")]
        root: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long = "export-format", value_enum, default_value_t)]
        export_format: ExportFormatArg,
    },
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum ExportFormatArg {
    #[default]
    Json,
    Jsonl,
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
    if let Some(code) = maybe_run_with_pager() {
        return code;
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let exit = error.exit_code().clamp(0, 255) as u8;
            let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
            if exit != 0 && requests_json(&arguments) {
                let _ = output::emit_typed_error(
                    "cli",
                    "invalid_argument",
                    &error.to_string(),
                    false,
                    Some("run the command with --help to inspect its accepted parameters"),
                    &[],
                );
            } else {
                let _ = error.print();
            }
            return ExitCode::from(exit);
        }
    };
    let format = cli.format;
    let command = cli.command.name();
    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            let (error_code, exit) = classify_error(error.as_ref());
            if format.is_json() {
                let _ = output::emit_typed_error(
                    command,
                    error_code,
                    &error.to_string(),
                    exit == 4,
                    None,
                    &[],
                );
            } else {
                eprintln!("error[{error_code}]: {error}");
            }
            ExitCode::from(exit)
        }
    }
}

fn maybe_run_with_pager() -> Option<ExitCode> {
    let mut arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut choice = PagerChoice::Auto;
    let mut index = 0usize;
    while index < arguments.len() {
        let argument = arguments[index].to_string_lossy();
        if let Some(value) = argument.strip_prefix("--pager=") {
            choice = match value {
                "always" => PagerChoice::Always,
                "never" => PagerChoice::Never,
                _ => PagerChoice::Auto,
            };
            arguments.remove(index);
            continue;
        }
        if argument == "--pager" && index + 1 < arguments.len() {
            choice = match arguments[index + 1].to_string_lossy().as_ref() {
                "always" => PagerChoice::Always,
                "never" => PagerChoice::Never,
                _ => PagerChoice::Auto,
            };
            arguments.drain(index..=index + 1);
            continue;
        }
        index += 1;
    }
    if choice == PagerChoice::Never
        || choice == PagerChoice::Auto && !std::io::stdout().is_terminal()
        || requests_json(&arguments)
        || arguments.iter().any(|argument| {
            matches!(
                argument.to_string_lossy().as_ref(),
                "daemon" | "lsp" | "preview"
            )
        })
    {
        return None;
    }
    arguments.extend([OsString::from("--pager"), OsString::from("never")]);
    let mut pager = if cfg!(windows) {
        std::process::Command::new("more.com")
    } else {
        let executable = std::env::var_os("PAGER").unwrap_or_else(|| OsString::from("less"));
        let mut command = std::process::Command::new(executable);
        command.args(["-F", "-R", "-X"]);
        command
    };
    let mut pager = pager.stdin(Stdio::piped()).spawn().ok()?;
    let mut child = std::process::Command::new(std::env::current_exe().ok()?)
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .ok()?;
    let mut output = child.stdout.take()?;
    let mut pager_input = pager.stdin.take()?;
    let _ = std::io::copy(&mut output, &mut pager_input);
    drop(pager_input);
    let status = child.wait().ok()?;
    let _ = pager.wait();
    Some(ExitCode::from(
        status.code().unwrap_or(70).clamp(0, 255) as u8
    ))
}

fn requests_json(arguments: &[OsString]) -> bool {
    arguments
        .windows(2)
        .any(|pair| pair[0] == "--format" && pair[1] == "json")
        || arguments.iter().any(|argument| argument == "--format=json")
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
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::AlreadyExists => {
                    ("edit_conflict", 5)
                }
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
    let _ = cli.pager;
    official_docs::ensure_synced()?;
    match cli.command {
        Command::Daemon {
            action: Some(DaemonAction::Stop { root }),
            ..
        } => {
            service::stop_daemon(resolve_vault_root(&root)?, cli.format)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Daemon {
            root,
            background_child,
            ..
        } => service::run_daemon(resolve_vault_root(&root)?, background_child, cli.format),
        Command::Lsp => {
            require_protocol_format(cli.format, "lsp")?;
            lsp::run(cli.no_daemon)
        }
        Command::Skill {
            command: SkillCommand::Init { output, force },
        } => {
            let output = skill::init(output, force)?;
            if cli.format.is_json() {
                output::emit_result(
                    "skill init",
                    true,
                    serde_json::json!({"output": output, "files": ["SKILL.md"]}),
                )?;
            } else {
                println!("initialized Notist Skill at {}", output.display());
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Status { root } => {
            let (root, mut client, view_id) = connect_cli(root, cli.no_daemon)?;
            let reply = client.request(CoreRequest::Status { view_id })?;
            let CoreResponse::Status(status) = reply.response else {
                return query_response_error("status", reply.response, cli.format);
            };
            if cli.format.is_json() {
                output::emit_result("status", true, &status)?;
            } else {
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
            }
            let _ = root;
            Ok(ExitCode::SUCCESS)
        }
        Command::Modules {
            root,
            prefix,
            kind,
            page,
        } => {
            let (_, mut client, view_id) = connect_cli(root, cli.no_daemon)?;
            let reply = client.request(CoreRequest::ListModules {
                view_id,
                query: notist_service::ModulesQuery {
                    prefix,
                    kind: kind.into(),
                    page: page.into(),
                },
            })?;
            let CoreResponse::Modules(page) = reply.response else {
                return query_response_error("modules", reply.response, cli.format);
            };
            if cli.format.is_json() {
                output::emit_result("modules", true, &page)?;
            } else {
                for item in &page.items {
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
                emit_continuation("modules", &page.page, &page.coverage);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Check {
            root,
            scope,
            summary,
            severity,
            page,
        } => {
            let root = resolve_vault_root(&root)?;
            let mut client =
                service::LocalNotistClient::connect(cli.no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::DiagnosticsPage {
                view_id,
                query: notist_service::DiagnosticsQuery {
                    scope,
                    summary_only: summary,
                    severity: severity.into(),
                    page: page.into(),
                },
            })?;
            let CoreResponse::DiagnosticsPage(result) = reply.response else {
                return query_response_error("check", reply.response, cli.format);
            };
            let ok = result.summary.error_count == 0;
            if cli.format.is_json() {
                output::emit_result("check", ok, &result)?;
            } else {
                emit_diagnostic_page(&result, cli.color);
            }
            if ok {
                if !cli.format.is_json() {
                    println!("checked {} sources", result.summary.checked_sources);
                }
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::FAILURE)
            }
        }
        Command::Search {
            query,
            root,
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
            page,
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
            let root = resolve_vault_root(&root)?;
            let mut client =
                service::LocalNotistClient::connect(cli.no_daemon, ClientKind::Cli, root.clone())?;
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
                    page: page.into(),
                },
            })?;
            let CoreResponse::SearchPage(results) = reply.response else {
                return query_response_error("search", reply.response, cli.format);
            };
            if cli.format.is_json() {
                output::emit_result("search", true, &results)?;
                return Ok(ExitCode::SUCCESS);
            }
            let group = results
                .search
                .as_ref()
                .map(|metadata| match metadata.group_by {
                    notist_service::SearchGroup::Source => "sources",
                    notist_service::SearchGroup::Section => "sections",
                    notist_service::SearchGroup::Match => "matches",
                })
                .unwrap_or("matches");
            if results.coverage.complete {
                println!("{} {group}", results.items.len());
            } else {
                println!("showing {} {group}; more available", results.items.len());
            }
            for (index, result) in results.items.iter().enumerate() {
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
                    "\n{}. {}  {} field={}{}",
                    index + 1,
                    result.location.module,
                    position,
                    result.matched_field,
                    score
                );
                println!("   {}", result.excerpt.replace('\n', " "));
            }
            for hint in &results.hints {
                if !hint.starts_with("more results:") {
                    println!("hint: {hint}");
                }
            }
            emit_continuation("search", &results.page, &results.coverage);
            let _ = root;
            Ok(ExitCode::SUCCESS)
        }
        Command::Outline {
            selector,
            root,
            depth,
            page,
        } => {
            let root = resolve_vault_root(&root)?;
            let mut client =
                service::LocalNotistClient::connect(cli.no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::OutlineModule {
                view_id,
                query: notist_service::OutlineQuery {
                    selector: notist_service::Selector::parse(&selector),
                    depth,
                    page: page.into(),
                },
            })?;
            let CoreResponse::OutlinePage(outline) = reply.response else {
                return query_response_error("outline", reply.response, cli.format);
            };
            if cli.format.is_json() {
                output::emit_result("outline", true, &outline)?;
                return Ok(ExitCode::SUCCESS);
            }
            for symbol in &outline.items {
                println!(
                    "{}{}  {}:{}",
                    "  ".repeat(symbol.level.saturating_sub(1) as usize),
                    symbol.name,
                    symbol.location.relative_path.display(),
                    symbol.location.line_range.map_or(0, |range| range.start)
                );
            }
            emit_continuation("outline", &outline.page, &outline.coverage);
            let _ = root;
            Ok(ExitCode::SUCCESS)
        }
        Command::Read {
            selector,
            root,
            from_line,
            lines,
            byte_range,
            page,
        } => {
            let (_, mut client, view_id) = connect_cli(root, cli.no_daemon)?;
            let reply = client.request(CoreRequest::ReadSource {
                view_id,
                query: notist_service::ReadQuery {
                    selector: notist_service::Selector::parse(&selector),
                    window: notist_service::ReadWindow {
                        from_line,
                        lines,
                        byte_range,
                    },
                    page: page.into(),
                },
            })?;
            let CoreResponse::SourcePage(result) = reply.response else {
                return query_response_error("read", reply.response, cli.format);
            };
            if cli.format.is_json() {
                output::emit_result("read", true, &result)?;
            } else if let Some(chunk) = result.items.first() {
                let start = chunk.location.line_range.map_or(1, |range| range.start);
                for (offset, line) in chunk.source.lines().enumerate() {
                    println!("{:>5} | {}", start + offset, line);
                }
                emit_continuation("read", &result.page, &result.coverage);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::References {
            selector,
            root,
            include_definition,
            direction,
            snippet_bytes,
            page,
        } => {
            let root = resolve_vault_root(&root)?;
            let mut client =
                service::LocalNotistClient::connect(cli.no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::ReferencesPage {
                view_id,
                query: notist_service::ReferencesQuery {
                    selector: notist_service::Selector::parse(&selector),
                    direction: direction.into(),
                    include_definition,
                    snippet_bytes,
                    page: page.into(),
                },
            })?;
            let CoreResponse::ReferencesPage(locations) = reply.response else {
                return query_response_error("references", reply.response, cli.format);
            };
            if cli.format.is_json() {
                output::emit_result("references", true, &locations)?;
                return Ok(ExitCode::SUCCESS);
            }
            for item in &locations.items {
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
            emit_continuation("references", &locations.page, &locations.coverage);
            let _ = root;
            Ok(ExitCode::SUCCESS)
        }
        Command::Query {
            query:
                QueryCommand::Definition {
                    path,
                    offset,
                    root,
                    expected_fingerprint,
                },
        } => {
            let root = resolve_vault_root(&root)?;
            let path = dunce::canonicalize(if path.is_absolute() {
                path
            } else {
                root.join(path)
            })?;
            let mut client =
                service::LocalNotistClient::connect(cli.no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::DefinitionLocation {
                view_id,
                query: notist_service::DefinitionQuery {
                    path: path.clone(),
                    offset,
                    expected_fingerprint,
                },
            })?;
            let snapshot = reply.snapshot.clone();
            let CoreResponse::DefinitionLocation(definition) = reply.response else {
                return query_response_error("query definition", reply.response, cli.format);
            };
            if cli.format.is_json() {
                output::emit_result(
                    "query definition",
                    true,
                    serde_json::json!({
                        "root": root,
                        "snapshot": snapshot,
                        "path": path,
                        "offset": offset,
                        "definition": definition,
                    }),
                )?;
                return Ok(ExitCode::SUCCESS);
            }
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
        Command::Edit {
            edit:
                EditCommand::Replace {
                    path,
                    start,
                    end,
                    replacement,
                    idempotency_key,
                    expected_fingerprint,
                    yes,
                },
        } => {
            let path = dunce::canonicalize(path)?;
            let root = resolve_vault_root(&path)?;
            let mut client =
                service::LocalNotistClient::connect(cli.no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let summary = client.request(CoreRequest::SnapshotSummary { view_id })?;
            let plan = client.request(CoreRequest::ProposeEdit {
                view_id,
                base_revision: summary.snapshot.revision,
                operations: vec![notist_service::EditOperation {
                    path,
                    range: notist_service::ByteRange { start, end },
                    replacement,
                }],
            })?;
            let CoreResponse::EditPlan(plan) = plan.response else {
                return Err("daemon returned an unexpected edit-plan response".into());
            };
            if plan
                .affected_sources
                .first()
                .is_none_or(|source| source.fingerprint != expected_fingerprint)
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "expected fingerprint does not match the captured source",
                )
                .into());
            }
            if !plan.diagnostics.is_empty() {
                if cli.format.is_json() {
                    output::emit_result(
                        "edit replace",
                        false,
                        serde_json::json!({"root": root, "plan": plan}),
                    )?;
                } else {
                    for diagnostic in plan.diagnostics {
                        eprintln!("notist edit: {diagnostic}");
                    }
                }
                return Ok(ExitCode::FAILURE);
            }
            if !yes {
                if cli.format.is_json() {
                    output::emit_result(
                        "edit replace",
                        false,
                        serde_json::json!({"root": root, "proposal": plan, "applied": false}),
                    )?;
                } else {
                    println!("proposed edit {}; pass --yes to apply", plan.plan_hash);
                    for preview in &plan.preview {
                        println!(
                            "\n--- {}:{}..{}\n- {}\n+ {}{}",
                            preview.path.display(),
                            preview.range.start,
                            preview.range.end,
                            preview.before.replace('\n', "\\n"),
                            preview.replacement.replace('\n', "\\n"),
                            if preview.truncated {
                                " (preview truncated)"
                            } else {
                                ""
                            }
                        );
                    }
                }
                return Ok(ExitCode::FAILURE);
            }
            let applied = client.request(CoreRequest::ApplyEdit {
                view_id,
                plan_hash: plan.plan_hash,
                expected_fingerprints: plan.affected_sources,
                idempotency_key,
            })?;
            let CoreResponse::EditApplied(applied) = applied.response else {
                return Err("daemon returned an unexpected edit response".into());
            };
            if cli.format.is_json() {
                output::emit_result(
                    "edit replace",
                    true,
                    serde_json::json!({"root": root, "applied": applied}),
                )?;
            } else {
                println!("applied edit {}", applied.plan_hash);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Edit {
            edit:
                EditCommand::Rename {
                    from,
                    to,
                    idempotency_key,
                    expected_fingerprint,
                    yes,
                },
        } => {
            let from = dunce::canonicalize(from)?;
            let root = resolve_vault_root(&from)?;
            let mut client =
                service::LocalNotistClient::connect(cli.no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let fingerprint = client.request(CoreRequest::FingerprintSource {
                view_id,
                path: from.clone(),
            })?;
            let CoreResponse::SourceFingerprint(Some(fingerprint)) = fingerprint.response else {
                return Err("source is not part of the selected vault".into());
            };
            if expected_fingerprint != fingerprint.fingerprint {
                return Err("expected fingerprint does not match the captured source".into());
            }
            if !yes {
                if cli.format.is_json() {
                    output::emit_result(
                        "edit rename",
                        false,
                        serde_json::json!({"root": root, "from": from, "to": to, "fingerprint": fingerprint, "applied": false}),
                    )?;
                } else {
                    println!(
                        "proposed rename {} -> {}; pass --yes to apply",
                        from.display(),
                        to.display()
                    );
                }
                return Ok(ExitCode::FAILURE);
            }
            let renamed = client.request(CoreRequest::RenameSource {
                view_id,
                from,
                to,
                expected_fingerprint: fingerprint.fingerprint,
                idempotency_key,
            })?;
            let CoreResponse::SourceRenamed(renamed) = renamed.response else {
                return Err("daemon returned an unexpected rename response".into());
            };
            if cli.format.is_json() {
                output::emit_result(
                    "edit rename",
                    true,
                    serde_json::json!({"root": root, "renamed": renamed}),
                )?;
            } else {
                println!(
                    "renamed {} -> {}",
                    renamed.from.display(),
                    renamed.to.display()
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Index { command } => {
            let (root, rebuild, wait) = match command {
                IndexCommand::Status { root } => (root, false, false),
                IndexCommand::Rebuild { root, wait } => (root, true, wait),
            };
            let (_, mut client, view_id) = connect_cli(root, cli.no_daemon)?;
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
                return query_response_error("index", reply.response, cli.format);
            };
            let submitted_operation = status.operation_handle.clone();
            if effective_wait && status.health == "building" {
                if cli.format.is_json() {
                    output::emit_event(
                        "index rebuild",
                        "waiting",
                        serde_json::json!({"operation_handle": status.operation_handle.clone()}),
                    )?;
                } else if let Some(operation) = &status.operation_handle {
                    eprintln!("waiting for {operation}");
                }
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    let polled = client.request(CoreRequest::IndexStatus { view_id })?;
                    let CoreResponse::IndexStatus(next) = polled.response else {
                        return query_response_error("index", polled.response, cli.format);
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
                return query_response_error("index", CoreResponse::QueryError(error), cli.format);
            }
            if cli.format.is_json() {
                output::emit_result("index", true, &status)?;
            } else {
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
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Debug {
            command:
                DebugCommand::Inspect {
                    root,
                    section,
                    module,
                    page,
                },
        } => {
            let (_, mut client, view_id) = connect_cli(root, cli.no_daemon)?;
            let section = match section {
                DebugSection::Modules => notist_service::DebugSection::Modules,
                DebugSection::References => notist_service::DebugSection::References,
                DebugSection::Semantic => notist_service::DebugSection::Semantic,
            };
            let reply = client.request(CoreRequest::DebugInspect {
                view_id,
                query: notist_service::DebugQuery {
                    section,
                    module,
                    page: page.into(),
                },
            })?;
            let CoreResponse::DebugPage(result) = reply.response else {
                return query_response_error("debug inspect", reply.response, cli.format);
            };
            if cli.format.is_json() {
                output::emit_result("debug inspect", true, &result)?;
            } else {
                for item in &result.items {
                    println!(
                        "{} {} {}",
                        item.module,
                        item.kind,
                        item.name
                            .as_deref()
                            .or(item.target.as_deref())
                            .unwrap_or("")
                    );
                }
                emit_continuation("debug inspect", &result.page, &result.coverage);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Export { command } => export_command(command, cli.no_daemon, cli.format),
        Command::Build {
            root,
            output,
            clean,
        } => build::run(
            resolve_vault_root(&root)?,
            output,
            cli.color,
            cli.no_daemon,
            clean,
            cli.format,
        ),
        Command::Preview {
            root,
            host,
            port,
            open,
        } => preview::run(
            resolve_vault_root(&root)?,
            host,
            port,
            open,
            cli.color,
            cli.no_daemon,
            cli.format,
        ),
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
    format: OutputFormat,
) -> Result<ExitCode, Box<dyn std::error::Error>> {
    if let CoreResponse::QueryError(error) = response {
        if format.is_json() {
            output::emit_typed_error(
                command,
                &error.code,
                &error.message,
                error.retryable,
                error.hint.as_deref(),
                &error.candidates,
            )?;
        } else {
            eprintln!("error[{}]: {}", error.code, error.message);
            if let Some(hint) = error.hint {
                eprintln!("hint: {hint}");
            }
            for candidate in error.candidates {
                eprintln!("  {candidate}");
            }
        }
        Ok(ExitCode::from(3))
    } else {
        Err(format!("service returned an unexpected {command} response").into())
    }
}

fn emit_continuation(
    _command: &str,
    page: &notist_service::PageInfo,
    coverage: &notist_service::CoverageInfo,
) {
    if let Some(cursor) = &page.next_cursor {
        println!("\nmore results available ({}).", coverage.stop_reason);
        println!("continue the same query with --cursor {cursor}");
    }
}

fn emit_diagnostic_page(result: &notist_service::DiagnosticsResult, color: clap::ColorChoice) {
    let color = matches!(color, clap::ColorChoice::Always)
        || matches!(color, clap::ColorChoice::Auto) && std::io::stderr().is_terminal();
    for diagnostic in &result.diagnostics.items {
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
        "{} diagnostics in {} sources; showing {}",
        result.summary.total_diagnostics,
        result.summary.checked_sources,
        result.diagnostics.page.returned
    );
    emit_continuation(
        "check",
        &result.diagnostics.page,
        &result.diagnostics.coverage,
    );
}

fn export_command(
    command: ExportCommand,
    no_daemon: bool,
    format: OutputFormat,
) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let (kind, root, output_path, artifact_format) = match command {
        ExportCommand::Diagnostics {
            root,
            output,
            export_format,
        } => ("diagnostics", root, output, export_format),
        ExportCommand::Snapshot {
            root,
            output,
            export_format,
        } => ("snapshot", root, output, export_format),
        ExportCommand::Outline {
            root,
            output,
            export_format,
        } => ("outline", root, output, export_format),
    };
    let (root, mut client, view_id) = connect_cli(root, no_daemon)?;
    let reply = match kind {
        "diagnostics" => client.request(CoreRequest::Diagnostics { view_id })?,
        "snapshot" => client.request(CoreRequest::Inspect { view_id })?,
        "outline" => client.request(CoreRequest::Outline { view_id })?,
        _ => unreachable!(),
    };
    let records = match &reply.response {
        CoreResponse::Diagnostics(items) => items.len(),
        CoreResponse::Inspect(value) => {
            value.modules.len() + value.references.len() + value.semantic_items.len()
        }
        CoreResponse::Outline(documents) => documents
            .iter()
            .map(|document| document.symbols.len())
            .sum(),
        _ => 0,
    };
    let snapshot = reply.snapshot.clone();
    let value = serde_json::to_value(&reply.response)?;
    let bytes = match artifact_format {
        ExportFormatArg::Json => serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2,
            "kind": kind,
            "snapshot": snapshot.clone(),
            "result": value,
        }))?,
        ExportFormatArg::Jsonl => {
            let mut lines: Vec<u8> = Vec::new();
            match &reply.response {
                CoreResponse::Diagnostics(items) => {
                    for item in items {
                        lines.extend_from_slice(&serde_json::to_vec(&serde_json::json!({
                            "schemaVersion": 2,
                            "kind": kind,
                            "snapshot": snapshot.clone(),
                            "item": item,
                        }))?);
                        lines.push(b'\n');
                    }
                }
                CoreResponse::Outline(documents) => {
                    for document in documents {
                        let module = document
                            .path
                            .strip_prefix(&root)
                            .unwrap_or(&document.path);
                        for symbol in &document.symbols {
                            lines.extend_from_slice(&serde_json::to_vec(&serde_json::json!({
                                "schemaVersion": 2,
                                "kind": kind,
                                "snapshot": snapshot.clone(),
                                "module": module,
                                "item": symbol,
                            }))?);
                            lines.push(b'\n');
                        }
                    }
                }
                CoreResponse::Inspect(_) => {
                    lines.extend_from_slice(&serde_json::to_vec(&serde_json::json!({
                        "schemaVersion": 2,
                        "kind": kind,
                        "snapshot": snapshot.clone(),
                        "result": value,
                    }))?);
                    lines.push(b'\n');
                }
                _ => {}
            }
            lines
        }
    };
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let checksum = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let stdout_artifact = output_path.as_os_str() == "-";
    if stdout_artifact {
        println!("{}", String::from_utf8(bytes.clone())?);
    } else {
        let parent = output_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(PathBuf::from(".").as_path())
            .to_path_buf();
        std::fs::create_dir_all(&parent)?;
        notist_service::write_artifact_atomic(
            &output_path,
            &bytes,
            &format!("export-{}", std::process::id()),
        )?;
    }
    if !stdout_artifact && format.is_json() {
        output::emit_result(
            "export",
            true,
            serde_json::json!({
                "kind": kind,
                "root": root,
                "output": output_path,
                "records": records,
                "bytes": bytes.len(),
                "checksum": checksum,
                "snapshot": snapshot,
            }),
        )?;
    } else if !stdout_artifact {
        println!(
            "wrote {} records, {} bytes -> {}\nchecksum: {}",
            records,
            bytes.len(),
            output_path.display(),
            checksum
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn require_protocol_format(
    format: OutputFormat,
    protocol: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if format.is_json() {
        return Err(format!(
            "`{protocol}` already uses JSON-RPC on stdout and does not accept `--format json`"
        )
        .into());
    }
    Ok(())
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
