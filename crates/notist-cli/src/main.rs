use std::net::IpAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use notist_analysis::resolve_vault_root;
use notist_service::protocol::ClientKind;
use notist_service::{CoreRequest, CoreResponse, ProtocolViewKind};

mod build;
mod convert;
mod lsp;
mod mcp;
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

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the shared local Notist daemon for one vault.
    Daemon {
        /// Root directory of the vault this daemon serves.
        #[arg(default_value = ".")]
        root: PathBuf,
        #[arg(long, hide = true)]
        background_child: bool,
    },
    /// Run the Notist language server over standard input and output.
    Lsp,
    /// Run the Notist MCP server over standard input and output.
    Mcp {
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// Create resources that teach an Agent how to use Notist.
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Check module paths and references in a Notist workspace.
    Check {
        /// Root directory of the Notist workspace.
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// Print discovered modules and resolved references.
    Inspect {
        /// Root directory of the Notist workspace.
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// Search captured source context in a vault.
    Search {
        query: String,
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// Print the evaluated heading outline for a vault.
    Outline {
        #[arg(default_value = ".")]
        root: PathBuf,
    },
    /// Find references to a logical module.
    References {
        /// Absolute logical module path such as `vault::designs::D0011`.
        module: String,
        #[arg(default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        include_definition: bool,
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
    /// Convert a Markdown or Obsidian vault into a Notist vault.
    Convert {
        /// Root directory of the Markdown vault.
        source: PathBuf,
        /// Directory in which to create the Notist vault.
        output: PathBuf,
        /// Allow writing into an existing output directory.
        #[arg(long)]
        force: bool,
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
        /// Do not open the preview URL in the default browser.
        #[arg(long)]
        no_open: bool,
    },
}

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Self::Daemon { .. } => "daemon",
            Self::Lsp => "lsp",
            Self::Mcp { .. } => "mcp",
            Self::Skill { .. } => "skill init",
            Self::Check { .. } => "check",
            Self::Inspect { .. } => "inspect",
            Self::Search { .. } => "search",
            Self::Outline { .. } => "outline",
            Self::References { .. } => "references",
            Self::Query { .. } => "query definition",
            Self::Edit {
                edit: EditCommand::Replace { .. },
            } => "edit replace",
            Self::Edit {
                edit: EditCommand::Rename { .. },
            } => "edit rename",
            Self::Build { .. } => "build",
            Self::Convert { .. } => "convert",
            Self::Preview { .. } => "preview",
        }
    }
}

#[derive(Debug, Subcommand)]
enum QueryCommand {
    /// Find the definition at a source byte offset.
    Definition { path: PathBuf, offset: usize },
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
    },
    /// Rename a source while preserving its stable file identity.
    Rename {
        from: PathBuf,
        to: PathBuf,
        #[arg(long)]
        idempotency_key: String,
    },
}

#[derive(Debug, Subcommand)]
enum SkillCommand {
    /// Initialize the official Notist Skill in a new directory.
    Init { output: PathBuf },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let format = cli.format;
    let command = cli.command.name();
    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            if format.is_json() {
                let _ = output::emit_error(command, &error.to_string());
            } else {
                output::emit_text_error(&format!("notist: {error}"));
            }
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, Box<dyn std::error::Error>> {
    official_docs::ensure_synced()?;
    match cli.command {
        Command::Daemon {
            root,
            background_child,
        } => service::run_daemon(resolve_vault_root(&root)?, background_child, cli.format),
        Command::Lsp => {
            require_protocol_format(cli.format, "lsp")?;
            lsp::run(cli.no_daemon)
        }
        Command::Mcp { root } => {
            require_protocol_format(cli.format, "mcp")?;
            mcp::run(resolve_vault_root(&root)?, cli.no_daemon)
        }
        Command::Skill {
            command: SkillCommand::Init { output },
        } => {
            let output = skill::init(output)?;
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
        Command::Check { root } => {
            let root = resolve_vault_root(&root)?;
            let mut client =
                service::LocalNotistClient::connect(cli.no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::Diagnostics { view_id })?;
            let snapshot = reply.snapshot.clone();
            let CoreResponse::Diagnostics(diagnostics) = reply.response else {
                return Err("daemon returned an unexpected diagnostics response".into());
            };
            let summary_reply = client.request(CoreRequest::SnapshotSummary { view_id })?;
            let CoreResponse::SnapshotSummary(summary) = summary_reply.response else {
                return Err("daemon returned an unexpected snapshot response".into());
            };
            let ok = diagnostics.is_empty();
            if cli.format.is_json() {
                output::emit_result(
                    "check",
                    ok,
                    serde_json::json!({
                        "root": root,
                        "snapshot": snapshot,
                        "summary": summary,
                        "diagnostics": diagnostics,
                    }),
                )?;
            } else {
                emit_service_diagnostics(&diagnostics);
            }
            if ok {
                if !cli.format.is_json() {
                    println!("checked {} modules", summary.module_count);
                }
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::FAILURE)
            }
        }
        Command::Inspect { root } => {
            let root = resolve_vault_root(&root)?;
            let mut client =
                service::LocalNotistClient::connect(cli.no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::Inspect { view_id })?;
            let snapshot = reply.snapshot.clone();
            let CoreResponse::Inspect(inspect) = reply.response else {
                return Err("daemon returned an unexpected inspect response".into());
            };
            if cli.format.is_json() {
                output::emit_result(
                    "inspect",
                    true,
                    serde_json::json!({"root": root, "snapshot": snapshot, "inspect": inspect}),
                )?;
                return Ok(ExitCode::SUCCESS);
            }
            for module in inspect.modules {
                match module.source_path {
                    Some(path) => println!("{} -> {}", module.logical_path, path.display()),
                    None => println!("{} -> <virtual>", module.logical_path),
                }
            }
            for reference in inspect.references {
                println!(
                    "{}:{}..{} => {}",
                    reference.source_module,
                    reference.range.start,
                    reference.range.end,
                    reference.target_module
                );
            }
            for item in inspect.semantic_items {
                let suffix = item.name.map_or_else(String::new, |name| {
                    if item.kind == "embedded" {
                        format!(" @{name}")
                    } else {
                        format!(" {name}")
                    }
                });
                println!(
                    "{}:{}..{} {}{}",
                    item.module, item.range.start, item.range.end, item.kind, suffix
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Search { query, root } => {
            let root = resolve_vault_root(&root)?;
            let mut client =
                service::LocalNotistClient::connect(cli.no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::Search {
                view_id,
                query: query.clone(),
            })?;
            let snapshot = reply.snapshot.clone();
            let CoreResponse::Search(results) = reply.response else {
                return Err("daemon returned an unexpected search response".into());
            };
            if cli.format.is_json() {
                output::emit_result(
                    "search",
                    true,
                    serde_json::json!({
                        "root": root,
                        "snapshot": snapshot,
                        "query": query,
                        "results": results,
                    }),
                )?;
                return Ok(ExitCode::SUCCESS);
            }
            for result in results {
                println!(
                    "{}:{}..{} {}",
                    result.path.display(),
                    result.range.start,
                    result.range.end,
                    result.snippet
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Outline { root } => {
            let root = resolve_vault_root(&root)?;
            let mut client =
                service::LocalNotistClient::connect(cli.no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::Outline { view_id })?;
            let snapshot = reply.snapshot.clone();
            let CoreResponse::Outline(outline) = reply.response else {
                return Err("daemon returned an unexpected outline response".into());
            };
            if cli.format.is_json() {
                output::emit_result(
                    "outline",
                    true,
                    serde_json::json!({"root": root, "snapshot": snapshot, "documents": outline}),
                )?;
                return Ok(ExitCode::SUCCESS);
            }
            for document in outline {
                for symbol in document.symbols {
                    println!(
                        "{}:{}..{} {}{}",
                        document.path.display(),
                        symbol.range.start,
                        symbol.range.end,
                        "  ".repeat(symbol.level.saturating_sub(1) as usize),
                        symbol.name
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::References {
            module,
            root,
            include_definition,
        } => {
            let root = resolve_vault_root(&root)?;
            let mut client =
                service::LocalNotistClient::connect(cli.no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::ReferencesTo {
                view_id,
                module: module.clone(),
                include_definition,
            })?;
            let snapshot = reply.snapshot.clone();
            let CoreResponse::References(locations) = reply.response else {
                return Err("daemon returned an unexpected references response".into());
            };
            if cli.format.is_json() {
                output::emit_result(
                    "references",
                    true,
                    serde_json::json!({
                        "root": root,
                        "snapshot": snapshot,
                        "module": module,
                        "include_definition": include_definition,
                        "locations": locations,
                    }),
                )?;
                return Ok(ExitCode::SUCCESS);
            }
            for location in locations {
                println!(
                    "{}:{}..{}{}",
                    location.path.display(),
                    location.range.start,
                    location.range.end,
                    if location.is_definition {
                        " definition"
                    } else {
                        ""
                    }
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Query {
            query: QueryCommand::Definition { path, offset },
        } => {
            let path = dunce::canonicalize(path)?;
            let root = resolve_vault_root(&path)?;
            let mut client =
                service::LocalNotistClient::connect(cli.no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root.clone())?;
            let reply = client.request(CoreRequest::Definition {
                view_id,
                path: path.clone(),
                offset,
            })?;
            let snapshot = reply.snapshot.clone();
            let CoreResponse::Definition(definition) = reply.response else {
                return Err("daemon returned an unexpected definition response".into());
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
                    definition.path.display(),
                    definition.range.start,
                    definition.range.end
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
            if !plan.diagnostics.is_empty() {
                if cli.format.is_json() {
                    output::emit_result(
                        "edit replace",
                        false,
                        serde_json::json!({"root": root, "plan": plan}),
                    )?;
                } else {
                    for diagnostic in plan.diagnostics {
                        output::emit_text_error(&format!("notist edit: {diagnostic}"));
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
        Command::Convert {
            source,
            output,
            force,
        } => {
            let result = convert::run(&source, &output, force)?;
            if cli.format.is_json() {
                output::emit_result("convert", true, serde_json::to_value(&result)?)?;
            } else {
                println!(
                    "converted {} Markdown files and copied {} assets to {}",
                    result.converted_files,
                    result.copied_assets,
                    result.output.display()
                );
                for warning in &result.warnings {
                    output::emit_text_error(&format!("notist convert: warning: {warning}"));
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Preview {
            root,
            host,
            port,
            no_open,
        } => preview::run(
            resolve_vault_root(&root)?,
            host,
            port,
            no_open,
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
        output::emit_text_error(&format!(
            "{path}{range}: {} [{}]",
            diagnostic.message, diagnostic.code
        ));
    }
}
