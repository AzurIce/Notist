use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ColorChoice as ClapColorChoice;
use codespan_reporting::diagnostic::{Diagnostic as CodespanDiagnostic, Label};
use codespan_reporting::files::SimpleFiles;
use codespan_reporting::term;
use codespan_reporting::term::termcolor::{ColorChoice, StandardStream};
use notist_analysis::Workspace;
use notist_eval::EvalDiagnostic;

pub fn emit(workspace: &Workspace, color: ClapColorChoice) -> Result<(), Box<dyn Error>> {
    let mut files = SimpleFiles::new();
    let mut file_ids = HashMap::<PathBuf, usize>::new();

    for diagnostic in workspace.diagnostics() {
        let Some(path) = diagnostic.source_path.as_ref() else {
            continue;
        };
        if file_ids.contains_key(path) {
            continue;
        }

        let source = fs::read_to_string(path)?;
        let name = display_path(path);
        let id = files.add(name, source);
        file_ids.insert(path.clone(), id);
    }

    let writer = StandardStream::stderr(to_termcolor(color));
    let mut writer = writer.lock();
    let config = term::Config {
        tab_width: 4,
        ..term::Config::default()
    };

    for diagnostic in workspace.diagnostics() {
        let mut rendered = CodespanDiagnostic::error().with_message(&diagnostic.message);
        if let (Some(path), Some(range)) = (&diagnostic.source_path, diagnostic.range)
            && let Some(&file_id) = file_ids.get(path)
        {
            let source_len = files.get(file_id)?.source().len();
            let start = range.start.min(source_len);
            let end = range.end.min(source_len).max(start);
            rendered = rendered.with_labels(vec![Label::primary(file_id, start..end)]);
        } else if let Some(path) = &diagnostic.source_path {
            rendered = rendered.with_notes(vec![format!("at {}", display_path(path))]);
        }

        term::emit_to_write_style(&mut writer, &config, &files, &rendered)?;
    }

    Ok(())
}

pub fn emit_evaluation(
    path: &Path,
    source: &str,
    diagnostics: &[EvalDiagnostic],
    color: ClapColorChoice,
) -> Result<(), Box<dyn Error>> {
    let mut files = SimpleFiles::new();
    let file_id = files.add(display_path(path), source);
    let writer = StandardStream::stderr(to_termcolor(color));
    let mut writer = writer.lock();
    let config = term::Config {
        tab_width: 4,
        ..term::Config::default()
    };

    for diagnostic in diagnostics {
        let source_len = files.get(file_id)?.source().len();
        let start = diagnostic.range.start.min(source_len);
        let end = diagnostic.range.end.min(source_len).max(start);
        let rendered = CodespanDiagnostic::error()
            .with_message(&diagnostic.message)
            .with_labels(vec![Label::primary(file_id, start..end)]);
        term::emit_to_write_style(&mut writer, &config, &files, &rendered)?;
    }

    Ok(())
}

fn display_path(path: &Path) -> String {
    env::current_dir()
        .ok()
        .and_then(|current| path.strip_prefix(current).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn to_termcolor(color: ClapColorChoice) -> ColorChoice {
    match color {
        ClapColorChoice::Auto => ColorChoice::Auto,
        ClapColorChoice::Always => ColorChoice::Always,
        ClapColorChoice::Never => ColorChoice::Never,
    }
}
