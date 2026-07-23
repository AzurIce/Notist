use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::ColorChoice;
use notist_analysis::Workspace;
use notist_eval::{EvalDiagnostic, Evaluator, structure};
use notist_html::{RenderOptions, render_with_resolvers};
use notist_model::{ModulePath, TextRange};
use percent_encoding::{AsciiSet, CONTROLS, NON_ALPHANUMERIC, utf8_percent_encode};

use crate::diagnostics;

const URL_PATH_SEGMENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'%')
    .add(b'/')
    .add(b'\\')
    .add(b'?')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'"');

pub fn run(root: PathBuf, output: PathBuf, color: ColorChoice) -> Result<ExitCode, Box<dyn Error>> {
    let workspace = Workspace::load(root)?;
    let output = prepare_output_root(&output, workspace.root())?;
    let result = build_site(&workspace, &output, SiteOptions::default())?;

    emit_diagnostics(&workspace, &result, color)?;

    let diagnostic_count = diagnostic_count(&workspace, &result);
    if diagnostic_count == 0 {
        println!(
            "built {} pages -> {}",
            result.page_count,
            display_path(&output)
        );
        Ok(ExitCode::SUCCESS)
    } else {
        println!(
            "built {} pages -> {} with {} diagnostics",
            result.page_count,
            display_path(&output),
            diagnostic_count
        );
        Ok(ExitCode::FAILURE)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SiteOptions {
    pub live_reload: bool,
}

pub(crate) struct BuildResult {
    pub page_count: usize,
    pub evaluation_reports: Vec<EvaluationReport>,
}

pub(crate) struct EvaluationReport {
    pub path: PathBuf,
    pub source: String,
    pub diagnostics: Vec<EvalDiagnostic>,
}

pub(crate) fn emit_diagnostics(
    workspace: &Workspace,
    result: &BuildResult,
    color: ColorChoice,
) -> Result<(), Box<dyn Error>> {
    diagnostics::emit(workspace, color)?;
    for report in &result.evaluation_reports {
        diagnostics::emit_evaluation(&report.path, &report.source, &report.diagnostics, color)?;
    }
    Ok(())
}

pub(crate) fn diagnostic_count(workspace: &Workspace, result: &BuildResult) -> usize {
    workspace.diagnostics().len()
        + result
            .evaluation_reports
            .iter()
            .map(|report| report.diagnostics.len())
            .sum::<usize>()
}

pub(crate) fn build_site(
    workspace: &Workspace,
    output: &Path,
    options: SiteOptions,
) -> Result<BuildResult, Box<dyn Error>> {
    fs::create_dir_all(output.join("_notist"))?;
    fs::write(output.join("_notist/style.css"), STYLES)?;
    if options.live_reload {
        fs::write(output.join("_notist/reload.js"), LIVE_RELOAD_SCRIPT)?;
    }

    let modules: Vec<_> = workspace.modules().collect();
    let known_modules: BTreeSet<_> = modules
        .iter()
        .map(|module| module.logical_path.clone())
        .collect();
    let site_name = workspace
        .root()
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Notist".into());
    let mut evaluation_reports = Vec::new();

    for module in &modules {
        let fragment = if let Some(source_path) = &module.source_path {
            let source = fs::read_to_string(source_path)?;
            let evaluation = Evaluator::default().evaluate(&source);
            let structured = structure(evaluation);
            let current = &module.logical_path;
            let resolver = |target: &ModulePath, label: Option<&str>| {
                known_modules
                    .contains(target)
                    .then(|| module_href(current, target, label))
            };
            let source_ids: Vec<_> = module
                .parse
                .as_ref()
                .into_iter()
                .flat_map(|parse| parse.annotations())
                .filter_map(|annotation| {
                    annotation
                        .attributes
                        .id
                        .as_ref()
                        .map(|id| (annotation.scope_range, id.value.clone()))
                })
                .collect();
            let source_id_resolver = |range: TextRange| {
                source_ids
                    .iter()
                    .find(|(scope_range, _)| {
                        scope_range.start <= range.start && range.end <= scope_range.end
                    })
                    .map(|(_, id)| id.clone())
            };
            let fragment = render_with_resolvers(
                &structured.document,
                &RenderOptions {
                    current_module: Some(current),
                    module_url_prefix: "",
                },
                &resolver,
                &source_id_resolver,
            );
            if !structured.diagnostics.is_empty() {
                evaluation_reports.push(EvaluationReport {
                    path: source_path.clone(),
                    source,
                    diagnostics: structured.diagnostics,
                });
            }
            fragment
        } else {
            virtual_module_fragment(&module.logical_path, &modules)
        };

        let html = page_shell(
            &site_name,
            &module.logical_path,
            &modules,
            &fragment,
            options,
        );
        let page_path = module_output_dir(output, &module.logical_path).join("index.html");
        if let Some(parent) = page_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(page_path, html)?;
    }

    Ok(BuildResult {
        page_count: modules.len(),
        evaluation_reports,
    })
}

fn page_shell(
    site_name: &str,
    current: &ModulePath,
    modules: &[&notist_analysis::Module],
    fragment: &str,
    options: SiteOptions,
) -> String {
    let mut escaped_site_name = String::new();
    escape_html(&mut escaped_site_name, site_name);
    let mut escaped_module = String::new();
    escape_html(&mut escaped_module, &current.to_string());
    let stylesheet = format!(
        "{}_notist/style.css",
        "../".repeat(current.segments().len())
    );
    let reload_script = options.live_reload.then(|| {
        format!(
            "<script src=\"{}_notist/reload.js\" defer></script>\n",
            "../".repeat(current.segments().len())
        )
    });
    let navigation = navigation(current, modules, site_name);

    format!(
        "<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{escaped_module} - {escaped_site_name}</title>\n<link rel=\"stylesheet\" href=\"{stylesheet}\">\n{}</head>\n<body>\n<div class=\"site-layout\">\n{navigation}\n<main class=\"page-main\">\n<header class=\"page-header\"><span>{escaped_module}</span></header>\n<article class=\"notist-document\">{fragment}</article>\n</main>\n</div>\n</body>\n</html>\n",
        reload_script.as_deref().unwrap_or_default()
    )
}

fn navigation(
    current: &ModulePath,
    modules: &[&notist_analysis::Module],
    site_name: &str,
) -> String {
    let mut output = String::from("<aside class=\"site-sidebar\"><div class=\"site-name\">");
    let root = ModulePath::root();
    output.push_str("<a href=\"");
    escape_attribute(&mut output, &module_href(current, &root, None));
    output.push_str("\">");
    escape_html(&mut output, site_name);
    output.push_str("</a></div><nav aria-label=\"Modules\"><ol class=\"module-list\">");

    for module in modules {
        let path = &module.logical_path;
        output.push_str("<li style=\"--module-depth:");
        output.push_str(&path.segments().len().to_string());
        output.push_str("\"><a href=\"");
        escape_attribute(&mut output, &module_href(current, path, None));
        if path == current {
            output.push_str("\" aria-current=\"page");
        }
        output.push_str("\">");
        if let Some(name) = path.segments().last() {
            escape_html(&mut output, name);
        } else {
            output.push_str("Home");
        }
        output.push_str("</a></li>");
    }

    output.push_str("</ol></nav></aside>");
    output
}

fn virtual_module_fragment(current: &ModulePath, modules: &[&notist_analysis::Module]) -> String {
    let mut output = String::from("<h1>");
    if let Some(name) = current.segments().last() {
        escape_html(&mut output, name);
    } else {
        output.push_str("Home");
    }
    output.push_str("</h1><ul class=\"module-index\">");

    let child_depth = current.segments().len() + 1;
    for module in modules {
        let path = &module.logical_path;
        if path.segments().len() == child_depth && path.segments().starts_with(current.segments()) {
            output.push_str("<li><a href=\"");
            escape_attribute(&mut output, &module_href(current, path, None));
            output.push_str("\">");
            escape_html(
                &mut output,
                path.segments().last().expect("child has a name"),
            );
            output.push_str("</a></li>");
        }
    }
    output.push_str("</ul>");
    output
}

fn module_output_dir(root: &Path, module: &ModulePath) -> PathBuf {
    module
        .segments()
        .iter()
        .fold(root.to_path_buf(), |path, segment| {
            path.join(filesystem_segment(segment))
        })
}

fn module_href(current: &ModulePath, target: &ModulePath, label: Option<&str>) -> String {
    let mut href = "../".repeat(current.segments().len());
    for segment in target.segments() {
        href.push_str(&url_path_segment(segment));
        href.push('/');
    }
    if href.is_empty() {
        href.push_str("./");
    }
    if let Some(label) = label {
        href.push('#');
        href.extend(utf8_percent_encode(label, NON_ALPHANUMERIC));
    }
    href
}

fn url_path_segment(segment: &str) -> String {
    utf8_percent_encode(&filesystem_segment(segment), URL_PATH_SEGMENT_ENCODE_SET).to_string()
}

fn filesystem_segment(segment: &str) -> String {
    let mut output = String::new();
    for character in segment.chars() {
        if character.is_control()
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '~' | '.'
            )
        {
            for byte in character.to_string().bytes() {
                write!(output, "~{byte:02X}").unwrap();
            }
        } else {
            output.push(character);
        }
    }

    if is_windows_reserved_name(&output) {
        output.insert_str(0, "~00");
    }
    output
}

fn is_windows_reserved_name(segment: &str) -> bool {
    let upper = segment.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || upper
            .strip_prefix("COM")
            .or_else(|| upper.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
}

fn prepare_output_root(output: &Path, workspace_root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    fs::create_dir_all(output)?;
    let output = dunce::canonicalize(output)?;
    if output == workspace_root {
        return Err("output directory must not be the workspace root".into());
    }
    Ok(output)
}

fn escape_html(output: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            _ => output.push(character),
        }
    }
}

fn escape_attribute(output: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
}

fn display_path(path: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|current| path.strip_prefix(current).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

const STYLES: &str = r#":root {
  color-scheme: light dark;
  font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  line-height: 1.65;
  color: #202124;
  background: #f7f8fa;
}
* { box-sizing: border-box; }
body { margin: 0; }
.site-layout {
  min-height: 100vh;
  display: grid;
  grid-template-columns: 260px minmax(0, 1fr);
}
.site-sidebar {
  position: sticky;
  top: 0;
  height: 100vh;
  overflow-y: auto;
  padding: 24px 16px;
  background: #eef0f2;
  border-right: 1px solid #d5d9dd;
}
.site-name {
  margin: 0 8px 18px;
  font-size: 18px;
  font-weight: 700;
}
.site-name a { color: inherit; text-decoration: none; }
.module-list { margin: 0; padding: 0; list-style: none; }
.module-list li { margin: 1px 0; }
.module-list a {
  display: block;
  min-height: 34px;
  padding: 7px 10px 7px calc(10px + var(--module-depth) * 14px);
  overflow-wrap: anywhere;
  color: #3c4043;
  text-decoration: none;
  border-radius: 4px;
}
.module-list a:hover { background: #e1e5e8; }
.module-list a[aria-current="page"] {
  color: #174ea6;
  background: #dbe7f8;
  font-weight: 600;
}
.page-main { min-width: 0; padding: 32px 48px 80px; }
.page-header {
  width: min(100%, 860px);
  margin: 0 auto 18px;
  color: #5f6368;
  font-size: 13px;
}
.notist-document {
  width: min(100%, 860px);
  margin: 0 auto;
  overflow-wrap: break-word;
}
h1, h2, h3, h4, h5, h6 {
  margin: 1.6em 0 0.55em;
  line-height: 1.25;
}
h1:first-child, h2:first-child, h3:first-child { margin-top: 0; }
p, ul, ol, dl, blockquote, pre { margin: 0 0 1em; }
dt { font-weight: 700; }
dd { margin: 0 0 0.75em 1.5em; }
.notist-task-list { padding-left: 0; list-style: none; }
.notist-task-item {
  display: grid;
  grid-template-columns: 18px minmax(0, 1fr);
  gap: 8px;
  align-items: start;
}
.notist-task-item > input { margin: 0.42em 0 0; }
.notist-task-item > p { margin-bottom: 0.5em; }
.notist-image {
  max-width: 100%;
  height: auto;
  vertical-align: middle;
}
.notist-figure { margin: 1.25em 0; }
.notist-figure figcaption { margin-top: 0.45em; color: var(--muted, #666); text-align: center; }
.notist-video { display: block; width: 100%; max-width: 100%; margin: 1.25em 0; }
.notist-audio { display: block; width: 100%; margin: 1em 0; }
.notist-math { font-family: "Cambria Math", "STIX Two Math", serif; }
div.notist-math { margin: 1em 0; overflow-x: auto; text-align: center; }
.notist-content abbr { text-decoration: underline dotted; text-underline-offset: 0.16em; cursor: help; }
.notist-citation { font-style: normal; white-space: nowrap; }
.notist-keyboard {
  padding: 0.08em 0.38em;
  border: 1px solid var(--border, #d0d7de);
  border-bottom-width: 2px;
  border-radius: 0.28em;
  background: var(--surface, #f6f8fa);
  font: 0.88em/1.35 ui-monospace, SFMono-Regular, Consolas, monospace;
  white-space: nowrap;
}
.notist-sample { font-family: ui-monospace, SFMono-Regular, Consolas, monospace; }
.notist-outline { margin: 1.2em 0; padding: 0.85em 1em; border-left: 3px solid var(--border, #d0d7de); background: var(--surface, #f6f8fa); }
.notist-outline ol { margin: 0; padding-left: 1.35em; }
.notist-outline li + li { margin-top: 0.25em; }
.notist-outline-level-2 { margin-left: 0.8em; }
.notist-outline-level-3 { margin-left: 1.6em; }
.notist-outline-level-4 { margin-left: 2.4em; }
.notist-outline-level-5 { margin-left: 3.2em; }
.notist-outline-level-6 { margin-left: 4em; }
.notist-spoiler {
  padding: 0 0.18em;
  border-radius: 0.18em;
  color: transparent;
  background: var(--text, #24292f);
  cursor: pointer;
  box-decoration-break: clone;
  -webkit-box-decoration-break: clone;
}
.notist-spoiler:hover, .notist-spoiler:focus {
  color: inherit;
  background: color-mix(in srgb, var(--text, #24292f) 12%, transparent);
  outline: 1px solid var(--border, #d0d7de);
}
.notist-callout {
  margin: 1em 0;
  padding: 0.75em 1em;
  border-inline-start: 0.28rem solid var(--accent, #4f46e5);
  border-radius: 0.25rem;
  background: color-mix(in srgb, var(--accent, #4f46e5) 8%, transparent);
}
.notist-callout-title { margin-bottom: 0.4em; font-weight: 600; }
.notist-callout > :last-child { margin-bottom: 0; }
.notist-details { margin: 1em 0; padding: 0.65em 0.85em; border: 1px solid var(--border); border-radius: 0.25rem; }
.notist-details summary { cursor: pointer; font-weight: 600; }
.notist-details > :last-child { margin-bottom: 0; }
.notist-footnote-ref { margin-inline: 0.12em; }
.notist-footnote-ref a { text-decoration: none; }
.notist-footnotes {
  margin-top: 2rem;
  padding-top: 0.75rem;
  border-top: 1px solid var(--border);
  font-size: 0.9em;
}
.notist-footnote-backref { margin-inline-start: 0.45em; }
.notist-table-wrapper {
  max-width: 100%;
  margin: 0 0 1em;
  overflow-x: auto;
}
.notist-content table {
  width: 100%;
  border-collapse: collapse;
  border-spacing: 0;
}
.notist-content th,
.notist-content td {
  min-width: 6rem;
  padding: 0.5em 0.7em;
  border: 1px solid var(--border, #d0d7de);
  vertical-align: top;
  text-align: start;
}
.notist-content th {
  background: var(--surface, #f6f8fa);
  font-weight: 600;
}
.notist-content th > :first-child,
.notist-content td > :first-child { margin-top: 0; }
.notist-content th > :last-child,
.notist-content td > :last-child { margin-bottom: 0; }
.notist-table-align-left { text-align: left; }
.notist-content table caption { padding: 0 0 0.5em; font-weight: 600; text-align: start; }
.notist-table-align-center { text-align: center; }
.notist-table-align-right { text-align: right; }
.notist-rule {
  margin: 1.75em 0;
  border: 0;
  border-top: 1px solid #c4c7c5;
}
.notist-pagebreak {
  margin: 1.75em 0;
  border: 0;
  border-top: 1px dashed #9aa0a6;
  break-after: page;
}
a {
  color: #1769aa;
  text-decoration-thickness: 1px;
  text-underline-offset: 3px;
}
blockquote {
  margin-left: 0;
  padding-left: 18px;
  border-left: 3px solid #9aa0a6;
  color: #4b4f52;
}
code, pre { font-family: "Cascadia Code", "SFMono-Regular", Consolas, monospace; }
code { font-size: 0.92em; }
pre {
  overflow-x: auto;
  padding: 16px 18px;
  background: #eef0f2;
  border: 1px solid #d5d9dd;
  border-radius: 4px;
}
.notist-reference-unresolved,
.notist-unresolved-call {
  text-decoration: underline wavy #b3261e;
  text-underline-offset: 3px;
}
@media (max-width: 760px) {
  .site-layout { display: block; }
  .site-sidebar {
    position: static;
    width: 100%;
    height: auto;
    max-height: 260px;
    border-right: 0;
    border-bottom: 1px solid #d5d9dd;
  }
  .page-main { padding: 28px 20px 56px; }
}
@media (prefers-color-scheme: dark) {
  :root { color: #e8eaed; background: #202124; }
  .site-sidebar { background: #292a2d; border-color: #3c4043; }
  .module-list a { color: #bdc1c6; }
  .module-list a:hover { background: #35363a; }
  .module-list a[aria-current="page"] { color: #aecbfa; background: #303f55; }
  .page-header { color: #9aa0a6; }
  a { color: #8ab4f8; }
  blockquote { color: #bdc1c6; border-left-color: #80868b; }
  pre { background: #292a2d; border-color: #5f6368; }
  .notist-rule { border-top-color: #5f6368; }
  .notist-pagebreak { border-top-color: #80868b; }
}
@media print {
  .notist-pagebreak {
    visibility: hidden;
    margin: 0;
    break-after: page;
  }
}
"#;

const LIVE_RELOAD_SCRIPT: &str = r#"(() => {
  const eventsUrl = new URL("events", document.currentScript.src);
  let revision;

  const events = new EventSource(eventsUrl);
  events.onmessage = (event) => {
    if (revision !== undefined && revision !== event.data) {
      location.reload();
      return;
    }
    revision = event.data;
  };
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_source_and_virtual_modules_with_relative_links() {
        let root = tempfile::TempDir::new().unwrap();
        fs::create_dir(root.path().join("notes")).unwrap();
        fs::write(
            root.path().join("README.not"),
            "#heading[Home]\n\nOpen [[guide]] and [[notes]].",
        )
        .unwrap();
        fs::write(
            root.path().join("guide.not"),
            "#heading[Guide]\n\nBack to [[vault]].",
        )
        .unwrap();
        fs::write(root.path().join("notes/chapter one.not"), "#heading[One]").unwrap();
        let workspace = Workspace::load(root.path()).unwrap();
        let output = root.path().join("site");

        let result = build_site(&workspace, &output, SiteOptions::default()).unwrap();

        assert_eq!(result.page_count, 4);
        assert!(result.evaluation_reports.is_empty());
        let home = fs::read_to_string(output.join("index.html")).unwrap();
        let guide = fs::read_to_string(output.join("guide/index.html")).unwrap();
        let notes = fs::read_to_string(output.join("notes/index.html")).unwrap();
        assert!(home.contains("href=\"guide/\""));
        assert!(guide.contains("href=\"../\""));
        assert!(notes.contains("href=\"../notes/chapter%20one/\""));
        assert!(output.join("notes/chapter one/index.html").is_file());
        assert!(output.join("_notist/style.css").is_file());
    }

    #[test]
    fn builds_annotation_ids_as_label_targets() {
        let root = tempfile::TempDir::new().unwrap();
        fs::write(
            root.path().join("README.not"),
            "[[guide#intro]]\n\n#heading[Home]@home",
        )
        .unwrap();
        fs::write(
            root.path().join("guide.not"),
            "#heading[Introduction]@intro",
        )
        .unwrap();
        let workspace = Workspace::load(root.path()).unwrap();
        let output = root.path().join("site");

        build_site(&workspace, &output, SiteOptions::default()).unwrap();

        let home = fs::read_to_string(output.join("index.html")).unwrap();
        let guide = fs::read_to_string(output.join("guide/index.html")).unwrap();
        assert!(home.contains("href=\"guide/#intro\""));
        assert!(home.contains("id=\"home\""));
        assert!(guide.contains("id=\"intro\""));
    }

    #[test]
    fn path_mapping_uses_clean_module_directories() {
        let root = Path::new("site");
        let module = ModulePath::from_segments(["designs".into(), "type system".into()]);

        assert_eq!(
            module_output_dir(root, &module),
            PathBuf::from("site/designs/type system")
        );
        assert_eq!(
            module_href(
                &ModulePath::from_segments(["designs".into(), "current".into()]),
                &module,
                None
            ),
            "../../designs/type%20system/"
        );
    }

    #[test]
    fn filesystem_mapping_escapes_windows_reserved_names_and_characters() {
        assert_eq!(filesystem_segment("CON"), "~00CON");
        assert_eq!(filesystem_segment("a:b.not"), "a~3Ab~2Enot");
        assert_eq!(url_path_segment("chapter one"), "chapter%20one");
    }

    #[test]
    fn refuses_to_build_over_the_workspace_root() {
        let root = tempfile::TempDir::new().unwrap();
        let canonical = dunce::canonicalize(root.path()).unwrap();

        let error = prepare_output_root(root.path(), &canonical).unwrap_err();

        assert!(error.to_string().contains("workspace root"));
    }
}
