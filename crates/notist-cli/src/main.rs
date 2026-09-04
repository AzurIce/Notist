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

#[derive(Debug, Parser)]
#[command(
    name = "notist",
    version,
    about,
    arg_required_else_help = true
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
    /// Investigate a Vault: read, refs.
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
    /// Read a module as attribute-annotated source: cut the selected range at
    /// governing annotation boundaries, merge adjacent pieces with equal
    /// effective attributes, and embed each uniform segment's source lines.
    Read {
        /// Exact ModulePath.
        module: String,
        /// Read one Item's range instead of the whole module.
        #[arg(long, conflicts_with_all = ["line", "offset", "byte_range", "from_line"])]
        item: Option<String>,
        /// 1-based inclusive line range START..END.
        #[arg(long, value_parser = parse_line_range, conflicts_with_all = ["offset", "byte_range", "from_line"])]
        line: Option<notist_service::LineRange>,
        /// Byte offset (zero-width point).
        #[arg(long, conflicts_with_all = ["line", "byte_range", "from_line"])]
        offset: Option<usize>,
        /// Byte range START..END (UTF-8, half-open).
        #[arg(long, value_parser = parse_byte_range, conflicts_with_all = ["line", "offset", "from_line"])]
        byte_range: Option<notist_service::ByteRange>,
        /// 1-based starting line of a window; without --lines reads to the source end.
        #[arg(long, conflicts_with_all = ["line", "offset", "byte_range"])]
        from_line: Option<usize>,
        /// Line count for the --from-line window.
        #[arg(long, requires = "from_line")]
        lines: Option<usize>,
        /// Project only the attribute environment; omit the source lines.
        #[arg(long)]
        attrs_only: bool,
        /// Show each entry's declaring annotation provenance (the layered
        /// ancestor view) instead of the merged effective Dict.
        #[arg(long)]
        origins: bool,
    },
    /// List the references that cross into a module or item's region from
    /// outside it.
    Refs {
        /// Exact ModulePath whose region's incoming references are listed.
        module: String,
        /// Restrict the region to one Item's canonical subtree.
        #[arg(long)]
        item: Option<String>,
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
        InspectCommand::Refs { module, item } => {
            let identity = item
                .as_deref()
                .map_or_else(|| module.clone(), |name| format!("{module}/{name}"));
            let (_, client, view_id) = connect_cli(vault.clone(), no_daemon)?;
            let reply = client.request(CoreRequest::RefsPage {
                view_id,
                query: notist_service::RefsQuery {
                    selector: notist_service::Selector { module, item },
                },
            })?;
            let CoreResponse::RefsPage(page) = reply.response else {
                return query_response_error("refs", reply.response);
            };
            // Same grammar as read: bold = structure words, cyan = every
            // identity, dim = metadata (positions, arrows, gutter), body =
            // authored source lines. Each row names the resolved target —
            // folding puts different items behind one region query.
            let palette = Palette::stdout(color);
            println!(
                "{}: {}",
                palette.cyan(&identity),
                palette.bold(&plural(page.records.len(), "reference"))
            );
            for (index, record) in page.records.iter().enumerate() {
                let position = record.location.line_range.map_or_else(
                    || {
                        format!(
                            "{}@{}..{}",
                            record.location.relative_path.display(),
                            record.location.byte_range.start,
                            record.location.byte_range.end
                        )
                    },
                    |range| format!("{}:{}", record.location.relative_path.display(), range.start),
                );
                println!(
                    "{} {} {} {}  {}",
                    palette.bold(&format!("[{}]", index + 1)),
                    palette.cyan(&format!("<{}>", record.target)),
                    palette.dim("<-"),
                    palette.cyan(&format!("<{}>", record.source)),
                    palette.dim(&position)
                );
                for line in &record.content {
                    println!(
                        "{} {} {}",
                        palette.dim(&format!("{:>5}", line.number)),
                        palette.dim("|"),
                        palette.body(&line.text),
                    );
                }
            }
            for hint in &page.hints {
                println!("hint: {hint}");
            }
            Ok(ExitCode::SUCCESS)
        }
        InspectCommand::Read {
            module,
            item,
            line,
            offset,
            byte_range,
            from_line,
            lines,
            attrs_only,
            origins,
        } => {
            let (_, client, view_id) = connect_cli(vault.clone(), no_daemon)?;
            let reply = client.request(CoreRequest::Region {
                view_id,
                query: notist_service::RegionQuery {
                    selector: notist_service::Selector { module, item },
                    offset,
                    byte_range,
                    line_range: line,
                    from_line,
                    lines,
                    include_content: !attrs_only,
                },
            })?;
            let CoreResponse::Region(page) = reply.response else {
                return query_response_error("read", reply.response);
            };
            let palette = Palette::stdout(color);
            for record in &page.records {
                print_region_record(record, &palette, origins);
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
        // The path handoff to the host editor: the identity-to-path bridge.
        palette.dim(&record.relative_path.display().to_string()),
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
