use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use notist_analysis::Workspace;
use notist_syntax::Scope;

mod diagnostics;

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
        Command::Check { root } => {
            let workspace = Workspace::load(root)?;
            diagnostics::emit(&workspace, cli.color)?;
            if workspace.diagnostics().is_empty() {
                println!("checked {} modules", workspace.modules().count());
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::FAILURE)
            }
        }
        Command::Inspect { root } => {
            let workspace = Workspace::load(root)?;
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
                for scope in &parse.scopes {
                    match scope {
                        Scope::Transparent(scope) => println!(
                            "{}:{}..{} transparent{}",
                            module.logical_path,
                            scope.body_range.start,
                            scope.body_range.end,
                            format_id(&scope.attributes)
                        ),
                        Scope::Opaque(scope) => println!(
                            "{}:{}..{} opaque {}{}",
                            module.logical_path,
                            scope.body_range.start,
                            scope.body_range.end,
                            scope.name.value,
                            format_id(&scope.attributes)
                        ),
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn format_id(attributes: &notist_syntax::Attributes) -> String {
    attributes
        .id
        .as_ref()
        .map(|id| format!(" @{}", id.value))
        .unwrap_or_default()
}
