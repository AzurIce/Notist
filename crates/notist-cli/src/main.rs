use std::net::IpAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use notist_analysis::resolve_vault_root;
use notist_service::protocol::ClientKind;
use notist_service::{CoreRequest, CoreResponse, ProtocolViewKind};

mod build;
mod lsp;
mod mcp;
mod official_docs;
mod preview;
mod resources;
mod service;
mod skill;

#[derive(Debug, Parser)]
#[command(name = "notist", version, about, arg_required_else_help = true)]
struct Cli {
    /// Control colored diagnostic output.
    #[arg(long, value_enum, default_value_t = clap::ColorChoice::Auto, global = true)]
    color: clap::ColorChoice,

    /// Run the application service in this process instead of using the local daemon.
    #[arg(long, global = true)]
    no_daemon: bool,

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
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("notist: {error}");
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
        } => service::run_daemon(resolve_vault_root(&root)?, background_child),
        Command::Lsp => lsp::run(cli.no_daemon),
        Command::Mcp { root } => mcp::run(resolve_vault_root(&root)?, cli.no_daemon),
        Command::Skill {
            command: SkillCommand::Init { output },
        } => {
            skill::init(output)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Check { root } => {
            let root = resolve_vault_root(&root)?;
            let mut client =
                service::LocalNotistClient::connect(cli.no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root)?;
            let reply = client.request(CoreRequest::Diagnostics { view_id })?;
            let CoreResponse::Diagnostics(diagnostics) = reply.response else {
                return Err("daemon returned an unexpected diagnostics response".into());
            };
            emit_service_diagnostics(&diagnostics);
            let summary = client.request(CoreRequest::SnapshotSummary { view_id })?;
            let CoreResponse::SnapshotSummary(summary) = summary.response else {
                return Err("daemon returned an unexpected snapshot response".into());
            };
            if diagnostics.is_empty() {
                println!("checked {} modules", summary.module_count);
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::FAILURE)
            }
        }
        Command::Inspect { root } => {
            let root = resolve_vault_root(&root)?;
            let mut client =
                service::LocalNotistClient::connect(cli.no_daemon, ClientKind::Cli, root.clone())?;
            let view_id = open_disk_view(&mut client, root)?;
            let reply = client.request(CoreRequest::Inspect { view_id })?;
            let CoreResponse::Inspect(inspect) = reply.response else {
                return Err("daemon returned an unexpected inspect response".into());
            };
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
            let view_id = open_disk_view(&mut client, root)?;
            let reply = client.request(CoreRequest::Search { view_id, query })?;
            let CoreResponse::Search(results) = reply.response else {
                return Err("daemon returned an unexpected search response".into());
            };
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
            let view_id = open_disk_view(&mut client, root)?;
            let reply = client.request(CoreRequest::Outline { view_id })?;
            let CoreResponse::Outline(outline) = reply.response else {
                return Err("daemon returned an unexpected outline response".into());
            };
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
            let view_id = open_disk_view(&mut client, root)?;
            let reply = client.request(CoreRequest::ReferencesTo {
                view_id,
                module,
                include_definition,
            })?;
            let CoreResponse::References(locations) = reply.response else {
                return Err("daemon returned an unexpected references response".into());
            };
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
            let view_id = open_disk_view(&mut client, root)?;
            let reply = client.request(CoreRequest::Definition {
                view_id,
                path,
                offset,
            })?;
            let CoreResponse::Definition(definition) = reply.response else {
                return Err("daemon returned an unexpected definition response".into());
            };
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
            let view_id = open_disk_view(&mut client, root)?;
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
                for diagnostic in plan.diagnostics {
                    eprintln!("notist edit: {diagnostic}");
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
            println!("applied edit {}", applied.plan_hash);
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
            let view_id = open_disk_view(&mut client, root)?;
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
            println!(
                "renamed {} -> {}",
                renamed.from.display(),
                renamed.to.display()
            );
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
        ),
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
