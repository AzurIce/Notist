use clap::{Args as ClapArgs, ValueEnum};
use codespan_reporting::{
    diagnostic::{Diagnostic as RenderDiagnostic, Label},
    files::SimpleFiles,
    term::{
        self,
        termcolor::{ColorChoice, StandardStream},
    },
};
use notist_analysis::ModuleProvider;
use notist_analysis::{Diagnostic, DiagnosticCode, ModuleKey, SourceSpan};
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Write},
    path::PathBuf,
};

#[derive(ClapArgs)]
pub struct Args {
    /// Package to check, including all loaded dependency modules.
    #[arg(default_value = ".")]
    pub package: PathBuf,
    /// Emit one structured report to stdout; diagnostics use UTF-8 byte spans.
    #[arg(long)]
    pub json: bool,
    /// Color in terminal diagnostics.
    #[arg(long, value_enum, default_value = "auto")]
    pub color: Color,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum Color {
    Auto,
    Always,
    Never,
}

pub fn run(args: Args) -> Result<bool, Box<dyn std::error::Error>> {
    let mut sources = BTreeMap::<String, String>::new();
    let mut files = SimpleFiles::<String, String>::new();
    let mut ids = BTreeMap::new();
    let mut diagnostics = vec![];
    let mut checked = 0;
    match notist_analysis::package::load_for_check(&args.package) {
        Ok(mut package) => {
            let snapshot = package.runtime.snapshot();
            let cwd = std::env::current_dir()?;
            for (source, input) in snapshot.sources() {
                let path = package
                    .paths
                    .get(source)
                    .map(|p| p.strip_prefix(&cwd).unwrap_or(p).display().to_string())
                    .unwrap_or_else(|| source.clone());
                sources.insert(source.clone(), path.clone());
                ids.insert(source.clone(), files.add(path, input.text.to_string()));
            }
            let mut query = snapshot.query();
            for error in snapshot.errors() {
                diagnostics.push(Diagnostic::error(DiagnosticCode::Setup, error, None));
            }
            for source in snapshot
                .sources()
                .keys()
                .filter(|_| snapshot.errors().is_empty())
            {
                let key = ModuleKey::from(snapshot.module_key(source).unwrap());
                let report = query.check(&key)?;
                checked += 1;
                for diagnostic in report.diagnostics() {
                    if !diagnostics.contains(&diagnostic) {
                        diagnostics.push(diagnostic);
                    }
                }
            }
        }
        Err(error) => diagnostics.push(Diagnostic::error(DiagnosticCode::Setup, error, None)),
    }
    let success = diagnostics.is_empty();
    if args.json {
        let mut stdout = io::stdout().lock();
        serde_json::to_writer_pretty(
            &mut stdout,
            &json!({"ok":success,"coverage":"executed_references","checked_modules":checked,"sources":sources,"diagnostics":diagnostics}),
        )?;
        writeln!(stdout)?;
    } else {
        let color = match args.color {
            Color::Always => ColorChoice::Always,
            Color::Never => ColorChoice::Never,
            Color::Auto
                if !io::stderr().is_terminal()
                    || std::env::var_os("NO_COLOR").is_some()
                    || std::env::var("TERM").is_ok_and(|s| s == "dumb") =>
            {
                ColorChoice::Never
            }
            Color::Auto => ColorChoice::Auto,
        };
        let writer = StandardStream::stderr(color);
        let mut writer = writer.lock();
        let config = term::Config::default();
        for d in diagnostics {
            let mut labels = vec![];
            if let Some(span) = &d.span
                && let Some(label) = label(span, true, &ids, &files)
            {
                labels.push(label);
            }
            for related in &d.related {
                if let Some(label) = label(&related.span, false, &ids, &files) {
                    labels.push(label.with_message(&related.message));
                }
            }
            let rendered = RenderDiagnostic::error()
                .with_code(d.code.as_str())
                .with_message(d.message)
                .with_labels(labels)
                .with_notes(d.notes);
            term::emit_to_write_style(&mut writer, &config, &files, &rendered)?;
        }
    }
    Ok(success)
}

fn label(
    span: &SourceSpan,
    primary: bool,
    ids: &BTreeMap<String, usize>,
    files: &SimpleFiles<String, String>,
) -> Option<Label<usize>> {
    let id = *ids.get(&span.source)?;
    let source = files.get(id).ok()?.source();
    if span.start > span.end
        || span.end > source.len()
        || !source.is_char_boundary(span.start)
        || !source.is_char_boundary(span.end)
    {
        return None;
    }
    Some(if primary {
        Label::primary(id, span.start..span.end)
    } else {
        Label::secondary(id, span.start..span.end)
    })
}
