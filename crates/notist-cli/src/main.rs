mod check;
mod eval;
mod lsp;
mod preview;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "notist", about = "Notist language tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Evaluate a package or source file (JSON output by default).
    Eval(eval::Args),
    /// Start the language server over stdio.
    Lsp,
    /// Serve a live package preview.
    Preview {
        #[arg(default_value = ".")]
        package: PathBuf,
        #[arg(default_value = "127.0.0.1:8000")]
        address: String,
    },
    /// Check all modules in a package.
    Check(check::Args),
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Command::Eval(args) => eval::run(args).map_err(Into::into),
        Command::Lsp => lsp::run(),
        Command::Preview { package, address } => preview::serve(&package, &address),
        Command::Check(args) => {
            if !check::run(args)? {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_arguments_before_execution() {
        for args in [
            vec!["notist", "eval"],
            vec!["notist", "eval", "example", "--html", "--snapshot"],
            vec!["notist", "eval", "example", "--request", "--bundle", "out"],
            vec!["notist", "eval", "example", "--bundle"],
            vec!["notist", "lsp", "unexpected"],
            vec!["notist", "check", "--unknown"],
        ] {
            assert!(Cli::try_parse_from(&args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn output_options_can_precede_the_input() {
        let cli = Cli::try_parse_from(["notist", "eval", "--html", "example.not"]).unwrap();
        let Command::Eval(args) = cli.command else {
            panic!("expected eval")
        };
        assert!(args.html);
        assert_eq!(args.path, PathBuf::from("example.not"));
    }
}
