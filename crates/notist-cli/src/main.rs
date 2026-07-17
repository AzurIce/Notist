use std::net::IpAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use notist_analysis::{Workspace, resolve_vault_root};
use notist_syntax::RawLiteralForm;

mod build;
mod diagnostics;
mod lsp;
mod preview;

#[derive(Debug, Parser)]
#[command(name = "notist", version, about, arg_required_else_help = true)]
struct Cli {
    /// Control colored diagnostic output.
    #[arg(long, value_enum, default_value_t = clap::ColorChoice::Auto, global = true)]
    color: clap::ColorChoice,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the Notist language server over standard input and output.
    Lsp,
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
    /// Build a Notist workspace as a multi-page static HTML site.
    Build {
        /// Root directory of the Notist workspace.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Directory to write the generated site.
        #[arg(short, long, default_value = "dist")]
        output: PathBuf,
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
    match cli.command {
        Command::Lsp => lsp::run(),
        Command::Check { root } => {
            let workspace = Workspace::load(resolve_vault_root(&root)?)?;
            diagnostics::emit(&workspace, cli.color)?;
            if workspace.diagnostics().is_empty() {
                println!("checked {} modules", workspace.modules().count());
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::FAILURE)
            }
        }
        Command::Inspect { root } => {
            let workspace = Workspace::load(resolve_vault_root(&root)?)?;
            for module in workspace.modules() {
                match &module.source_path {
                    Some(path) => println!("{} -> {}", module.logical_path, path.display()),
                    None => println!("{} -> <virtual>", module.logical_path),
                }
            }
            for reference in workspace.references() {
                println!(
                    "{}:{}..{} => {}",
                    reference.source_module,
                    reference.range.start,
                    reference.range.end,
                    reference.target_module
                );
            }
            for module in workspace.modules() {
                let Some(parse) = &module.parse else {
                    continue;
                };
                for annotation in parse.annotations() {
                    println!(
                        "{}:{}..{} embedded{}",
                        module.logical_path,
                        annotation.scope_range.start,
                        annotation.scope_range.end,
                        format_id(&annotation.attributes)
                    );
                }
                for call in parse.calls() {
                    let (range, kind) = match call.trailing.first() {
                        Some(body) => (body.payload_range, "content call"),
                        None => (call.range, "call"),
                    };
                    println!(
                        "{}:{}..{} {kind} {}",
                        module.logical_path, range.start, range.end, call.name.value,
                    );
                }
                for raw in parse.raw_literals() {
                    let form = match raw.form {
                        RawLiteralForm::Inline => "inline raw",
                        RawLiteralForm::Fenced => "fenced raw",
                    };
                    let tag = raw
                        .tag
                        .as_ref()
                        .map(|tag| format!(" {}", tag.value))
                        .unwrap_or_default();
                    println!(
                        "{}:{}..{} {form}{tag}",
                        module.logical_path, raw.payload_range.start, raw.payload_range.end,
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Build { root, output } => {
            build::run(resolve_vault_root(&root)?, output, cli.color)
        }
        Command::Preview {
            root,
            host,
            port,
            no_open,
        } => preview::run(resolve_vault_root(&root)?, host, port, no_open, cli.color),
    }
}

fn format_id(attributes: &notist_syntax::Attributes) -> String {
    attributes
        .id
        .as_ref()
        .map(|id| format!(" @{}", id.value))
        .unwrap_or_default()
}
