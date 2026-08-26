use std::error::Error;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::ColorChoice;
use notist_model::ModulePath;
use notist_plugin_host::PluginHtmlAssets;
use notist_service::protocol::ClientKind;
use notist_service::{
    AttributeRecord, CoreRequest, CoreResponse, InspectRecord, ProtocolViewKind,
    RenderedBindingRecord, RenderedHeadingRecord, RenderedWorkspaceRecord, ServiceViewId,
};
use percent_encoding::{AsciiSet, CONTROLS, NON_ALPHANUMERIC, utf8_percent_encode};

use crate::output::OutputFormat;
use crate::service::LocalNotistClient;

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

/// Module attribute key for explicit sibling ordering.
const NAV_ORDER_KEY: &str = "order";
/// Module tag (or bare id) that pins a module to the top of its sibling list.
const NAV_TOP_MARKER: &str = "top";

/// Returns whether a module's attributes mark it as pinned/top.
fn module_attributes_pinned(attributes: &[AttributeRecord]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.tags.iter().any(|tag| tag == NAV_TOP_MARKER)
            || attribute.id.as_deref() == Some(NAV_TOP_MARKER)
    })
}

/// Returns the explicit sibling order from a module's attributes.
fn module_attributes_order(attributes: &[AttributeRecord]) -> Option<i64> {
    attributes.iter().find_map(|attribute| {
        attribute.properties.iter().find_map(|(key, value)| {
            if key == NAV_ORDER_KEY {
                value.trim_matches('"').parse::<i64>().ok()
            } else {
                None
            }
        })
    })
}

/// Sort key used only by the CLI site/preview layer.
///
/// Pinned modules come first, then explicit `order` values ascending, then
/// modules without `order` (still deterministic by path). The vault root stays
/// at the top of rendered pages/navigation.
fn module_nav_sort_key(
    module_segments: &[String],
    attributes: &[AttributeRecord],
) -> (bool, bool, i64, ModulePath) {
    let path = ModulePath::from_segments(module_segments.to_vec());
    if module_segments.is_empty() {
        return (false, false, i64::MIN, path);
    }
    let pinned = module_attributes_pinned(attributes);
    let order = module_attributes_order(attributes);
    (!pinned, order.is_none(), order.unwrap_or(0), path)
}

/// Builds a module-path -> navigation attributes map from an Inspect record.
fn module_navigation_map(
    inspect: &InspectRecord,
) -> std::collections::BTreeMap<ModulePath, Vec<AttributeRecord>> {
    inspect
        .modules
        .iter()
        .map(|module| {
            let path = ModulePath::from_segments(
                module.logical_path.split("::").skip(1).map(str::to_owned),
            );
            (path, module.attributes.clone())
        })
        .collect()
}

pub fn run(
    root: PathBuf,
    output: PathBuf,
    _color: ColorChoice,
    no_daemon: bool,
    clean: bool,
    format: OutputFormat,
) -> Result<ExitCode, Box<dyn Error>> {
    let mut client = LocalNotistClient::connect(no_daemon, ClientKind::Cli, root.clone())?;
    let opened = client.request(CoreRequest::OpenView {
        root: root.clone(),
        kind: ProtocolViewKind::Disk,
    })?;
    let CoreResponse::Opened { view_id, .. } = opened.response else {
        return Err("service returned an unexpected open-view response".into());
    };
    let rendered = render_workspace(&mut client, view_id)?;
    let output = prepare_output_root(&output, &root)?;
    if clean {
        clean_output_root(&output)?;
    }
    let config_text = fs::read_to_string(root.join("Notist.toml")).ok();
    let plugin_assets = notist_plugin_host::plugin_html_assets(&root, config_text.as_deref())
        .map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid plugin manifest: {error}"),
            )
        })?;
    let site_styles = notist_plugin_host::site_styles(config_text.as_deref())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let style_web_paths = copy_site_styles(&root, &output, &site_styles)?;
    let result = write_rendered_site_with_plugins(
        &rendered,
        &output,
        SiteOptions::default(),
        &plugin_assets,
        &style_web_paths,
    )?;
    copy_plugin_assets(&root, &output)?;
    let mut diagnostics = rendered.analysis_diagnostics.clone();
    merge_diagnostics(&mut diagnostics, rendered.evaluation_diagnostics.clone());
    let error_count = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == "error")
        .count();
    let diagnostic_count = diagnostics.len();
    let ok = error_count == 0;
    if format.is_json() {
        crate::output::emit_result(
            "build",
            ok,
            serde_json::json!({
                "root": root,
                "output": output,
                "page_count": result.page_count,
                "diagnostics": diagnostics,
            }),
        )?;
        return Ok(if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    crate::emit_service_diagnostics(&diagnostics);

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

pub(crate) fn render_workspace(
    client: &mut LocalNotistClient,
    view_id: ServiceViewId,
) -> Result<RenderedWorkspaceRecord, Box<dyn Error>> {
    let reply = client.request(CoreRequest::RenderWorkspace { view_id })?;
    let CoreResponse::RenderedWorkspace(mut rendered) = reply.response else {
        return Err("service returned an unexpected render response".into());
    };

    // Navigation ordering is presentation concern of the CLI site/preview
    // layer, not part of the core render contract. Read module attributes
    // through Inspect and sort only the pages handed to site generation.
    let inspect_reply = client.request(CoreRequest::Inspect { view_id })?;
    let CoreResponse::Inspect(inspect) = inspect_reply.response else {
        return Err("service returned an unexpected inspect response".into());
    };
    let navigation = module_navigation_map(&inspect);
    rendered.pages.sort_by(|left, right| {
        let left_attributes = navigation
            .get(&ModulePath::from_segments(left.module_segments.clone()))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let right_attributes = navigation
            .get(&ModulePath::from_segments(right.module_segments.clone()))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        module_nav_sort_key(&left.module_segments, left_attributes).cmp(&module_nav_sort_key(
            &right.module_segments,
            right_attributes,
        ))
    });

    Ok(rendered)
}

/// Copies plugin package assets into the generated site.
///
/// This is a first concrete step toward per-plugin asset injection. It reads
/// the vault `Notist.toml`, and for each known plugin copies `assets/*` into
/// `_notist/plugins/<name>/`.
pub(crate) fn copy_plugin_assets(root: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let config_path = root.join("Notist.toml");
    if !config_path.is_file() {
        return Ok(());
    }
    let config_text = std::fs::read_to_string(&config_path)?;
    let packages = notist_plugin_host::plugin_package_dirs(root, Some(&config_text))?;
    let mut copied = 0usize;
    for (name, package_dir) in packages {
        let package_dir = notist_plugin_host::resolve_package_dir(&package_dir)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        let assets_dir = package_dir.join("assets");
        if !assets_dir.is_dir() {
            continue;
        }
        let target_dir = output.join("_notist/plugins").join(&name);
        fs::create_dir_all(&target_dir)?;
        for entry in std::fs::read_dir(&assets_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                fs::copy(&path, target_dir.join(entry.file_name()))?;
                tracing::debug!(
                    target: "notist_cli",
                    from = %path.display(),
                    to = %target_dir.join(entry.file_name()).display(),
                    "copied plugin asset"
                );
                copied += 1;
            }
        }
    }
    tracing::debug!(
        target: "notist_cli",
        files = copied,
        "plugin asset copy complete"
    );
    Ok(())
}

/// Copies `[site] styles` sheets into `_notist/styles/`, preserving the
/// declared relative structure, and returns their web paths for head links.
///
/// A style file that vanished mid-build degrades to a warning and drops out
/// of the page heads, mirroring the resource-copy policy.
pub(crate) fn copy_site_styles(
    root: &Path,
    output: &Path,
    styles: &[String],
) -> Result<Vec<String>, Box<dyn Error>> {
    let mut web_paths = Vec::new();
    for style in styles {
        let source = root.join(style);
        let target = output.join("_notist/styles").join(style);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Err(error) = fs::copy(&source, &target) {
            eprintln!(
                "warning: cannot copy site style `{}` to `{}`: {error}",
                source.display(),
                target.display()
            );
            continue;
        }
        tracing::debug!(
            target: "notist_cli",
            from = %source.display(),
            to = %target.display(),
            "copied site style"
        );
        web_paths.push(format!("_notist/styles/{style}"));
    }
    Ok(web_paths)
}

/// Test-only convenience wrapper; production paths pass an explicit plugin
/// asset set so page heads are generated from manifest contributions.
#[allow(dead_code)]
pub(crate) fn write_rendered_site(
    rendered: &RenderedWorkspaceRecord,
    output: &Path,
    options: SiteOptions,
) -> Result<RenderedBuildResult, Box<dyn Error>> {
    write_rendered_site_with_plugins(rendered, output, options, &[], &[])
}

pub(crate) fn write_rendered_site_with_plugins(
    rendered: &RenderedWorkspaceRecord,
    output: &Path,
    options: SiteOptions,
    plugin_assets: &[PluginHtmlAssets],
    site_styles: &[String],
) -> Result<RenderedBuildResult, Box<dyn Error>> {
    fs::create_dir_all(output.join("_notist"))?;
    fs::write(output.join("_notist/style.css"), STYLES)?;
    fs::write(output.join("_notist/site.js"), SITE_SCRIPT)?;
    if options.live_reload {
        fs::write(output.join("_notist/reload.js"), LIVE_RELOAD_SCRIPT)?;
        fs::write(output.join("_notist/inspect.js"), INSPECT_SCRIPT)?;
        fs::write(output.join("_notist/source.js"), SOURCE_SCRIPT)?;
    }
    let pages = rendered
        .pages
        .iter()
        .map(|page| PageView {
            module: ModulePath::from_segments(page.module_segments.clone()),
            title: page.title.as_deref(),
            headings: &page.headings,
            fragment: &page.fragment,
            bindings: &page.bindings,
            source: page.source.as_deref(),
            plugin_assets,
            site_styles,
        })
        .collect::<Vec<_>>();
    for page in &pages {
        let html = page_shell(&rendered.site_name, page, &pages, options);
        let page_path = module_output_dir(output, &page.module).join("index.html");
        if let Some(parent) = page_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&page_path, html)?;
        tracing::debug!(
            target: "notist_cli",
            page = %page_path.display(),
            "wrote page"
        );
    }
    // Resource files are copied wholesale next to their module pages under
    // their real file names (D0010 leaves the copy policy to the CLI). A
    // source file vanishing mid-build degrades to a warning, not a failure.
    for resource in &rendered.resources {
        let module = ModulePath::from_segments(resource.module_segments.clone());
        let target = module_output_dir(output, &module).join(&resource.name);
        if let Some(parent) = target.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            eprintln!(
                "warning: cannot create resource directory `{}`: {error}",
                parent.display()
            );
            continue;
        }
        if let Err(error) = fs::copy(&resource.source_path, &target) {
            eprintln!(
                "warning: cannot copy resource `{}` to `{}`: {error}",
                resource.source_path.display(),
                target.display()
            );
        }
    }
    Ok(RenderedBuildResult {
        page_count: pages.len(),
    })
}

pub(crate) struct RenderedBuildResult {
    pub page_count: usize,
}

pub(crate) fn merge_diagnostics(
    diagnostics: &mut Vec<notist_service::DiagnosticRecord>,
    additional: impl IntoIterator<Item = notist_service::DiagnosticRecord>,
) {
    for diagnostic in additional {
        if !diagnostics.iter().any(|existing| {
            existing.path == diagnostic.path
                && existing.range == diagnostic.range
                && existing.message == diagnostic.message
        }) {
            diagnostics.push(diagnostic);
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SiteOptions {
    pub live_reload: bool,
}

/// A rendered page plus the metadata the site chrome needs.
struct PageView<'a> {
    module: ModulePath,
    title: Option<&'a str>,
    headings: &'a [RenderedHeadingRecord],
    fragment: &'a str,
    bindings: &'a [RenderedBindingRecord],
    /// Raw `.not` source from the rendered snapshot, `None` for virtual
    /// directory modules. Only the preview shell embeds it.
    source: Option<&'a str>,
    /// Manifest-declared plugin HTML assets injected into the page head.
    plugin_assets: &'a [PluginHtmlAssets],
    /// Vault-declared site stylesheets (`[site] styles`), as web paths under
    /// `_notist/styles/`, linked after the built-in stylesheet.
    site_styles: &'a [String],
}

impl PageView<'_> {
    /// Human-facing page label: semantic title, last path segment, or `Home` for the vault root.
    fn label(&self) -> &str {
        self.title
            .or_else(|| self.module.segments().last().map(String::as_str))
            .unwrap_or("Home")
    }
}

fn page_shell(
    site_name: &str,
    page: &PageView<'_>,
    pages: &[PageView<'_>],
    options: SiteOptions,
) -> String {
    let depth = page.module.segments().len();
    let asset_prefix = "../".repeat(depth);
    let root_href = module_href(&page.module, &ModulePath::root(), None);

    let mut html = String::new();
    html.push_str(
        "<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>",
    );
    escape_html(&mut html, page.label());
    html.push_str(" · ");
    escape_html(&mut html, site_name);
    html.push_str("</title>\n<script>");
    html.push_str(THEME_BOOTSTRAP);
    html.push_str("</script>\n<link rel=\"stylesheet\" href=\"");
    html.push_str(&asset_prefix);
    html.push_str("_notist/style.css\">\n");
    // Vault-declared styles load after the built-in sheet so they can
    // override defaults (e.g. recolor annotated `.class` projections).
    for style in page.site_styles {
        let encoded = style
            .split('/')
            .map(|segment| utf8_percent_encode(segment, URL_PATH_SEGMENT_ENCODE_SET).to_string())
            .collect::<Vec<_>>()
            .join("/");
        html.push_str("<link rel=\"stylesheet\" href=\"");
        html.push_str(&asset_prefix);
        escape_attribute(&mut html, &encoded);
        html.push_str("\">\n");
    }
    html.push_str("<script src=\"");
    html.push_str(&asset_prefix);
    html.push_str("_notist/site.js\" defer></script>\n");
    for package in page.plugin_assets {
        for contribution in &package.contributions {
            let Some(component) = &contribution.web_component else {
                continue;
            };
            if let Some(style) = &component.style
                && let Some(file_name) = Path::new(style).file_name().and_then(|name| name.to_str())
            {
                html.push_str("<link rel=\"stylesheet\" href=\"");
                html.push_str(&asset_prefix);
                html.push_str("_notist/plugins/");
                html.push_str(&package.name);
                html.push('/');
                html.push_str(file_name);
                html.push_str("\">\n");
            }
            if let Some(file_name) = Path::new(&component.module)
                .file_name()
                .and_then(|name| name.to_str())
            {
                html.push_str("<script type=\"module\" src=\"");
                html.push_str(&asset_prefix);
                html.push_str("_notist/plugins/");
                html.push_str(&package.name);
                html.push('/');
                html.push_str(file_name);
                html.push_str("\"></script>\n");
            }
        }
    }
    if options.live_reload {
        html.push_str("<script src=\"");
        html.push_str(&asset_prefix);
        html.push_str("_notist/source.js\" defer></script>\n<script src=\"");
        html.push_str(&asset_prefix);
        html.push_str("_notist/reload.js\" defer></script>\n<script src=\"");
        html.push_str(&asset_prefix);
        html.push_str("_notist/inspect.js\" defer></script>\n");
    }
    html.push_str(
        "</head>\n<body>\n<a class=\"skip-link\" href=\"#page-content\">Skip to content</a>\n",
    );

    html.push_str("<header class=\"topbar\"><button class=\"icon-button\" id=\"nav-toggle\" type=\"button\" aria-label=\"Toggle navigation\" aria-expanded=\"false\" aria-controls=\"site-sidebar\">");
    html.push_str(ICON_MENU);
    html.push_str("</button><a class=\"topbar-site\" href=\"");
    escape_attribute(&mut html, &root_href);
    html.push_str("\">");
    escape_html(&mut html, site_name);
    html.push_str("</a></header>\n");

    if options.live_reload {
        // Preview toolbar: the source/rendered toggle, then the inspector
        // switch. Virtual directory modules have no `.not` source, so their
        // source toggle is disabled.
        html.push_str(
            "<div class=\"preview-chrome\"><button class=\"chrome-toggle\" id=\"source-toggle\" type=\"button\" role=\"switch\" aria-checked=\"false\"",
        );
        if page.source.is_some() {
            html.push_str(" aria-controls=\"source-panel\"");
        } else {
            html.push_str(" disabled title=\"This virtual module has no .not source file\"");
        }
        html.push_str(
            "><span class=\"chrome-switch\" aria-hidden=\"true\"></span><span>Source</span></button><button class=\"chrome-toggle\" id=\"inspect-toggle\" type=\"button\" role=\"switch\" aria-checked=\"false\"><span class=\"chrome-switch\" aria-hidden=\"true\"></span><span>Enhanced</span></button></div>\n",
        );
        // The module's root bindings, consumed by inspect.js for the Symbols
        // tab. `</` is escaped so a string value can never close the tag.
        let bindings_json = serde_json::to_string(page.bindings)
            .unwrap_or_else(|_| "[]".to_owned())
            .replace("</", "<\\/");
        html.push_str("<script type=\"application/json\" id=\"notist-bindings\">");
        html.push_str(&bindings_json);
        html.push_str("</script>\n");
        // Raw `.not` source for source.js, from the same snapshot that
        // produced the rendered fragment (never re-read from disk).
        if let Some(source) = page.source {
            let source_json = serde_json::to_string(source)
                .unwrap_or_else(|_| "null".to_owned())
                .replace("</", "<\\/");
            html.push_str("<script type=\"application/json\" id=\"notist-source\">");
            html.push_str(&source_json);
            html.push_str("</script>\n");
        }
    }

    html.push_str("<div class=\"site-layout\">\n<aside class=\"site-sidebar\" id=\"site-sidebar\" aria-label=\"Site navigation\"><div class=\"sidebar-header\"><a class=\"site-name\" href=\"");
    escape_attribute(&mut html, &root_href);
    html.push_str("\">");
    escape_html(&mut html, site_name);
    html.push_str(
        "</a><button class=\"icon-button theme-toggle\" id=\"theme-toggle\" type=\"button\" aria-label=\"Toggle color theme\">",
    );
    html.push_str(ICON_SUN);
    html.push_str(ICON_MOON);
    html.push_str("</button></div>\n");
    html.push_str(&module_tree(&page.module, pages));
    html.push_str("\n</aside>\n<div class=\"sidebar-scrim\"></div>\n<div class=\"page-body\">\n<main class=\"page-main\" id=\"page-content\">\n");
    html.push_str(&breadcrumb(site_name, page));
    html.push_str("<article class=\"notist-document\">");
    html.push_str(page.fragment);
    html.push_str("</article>\n");
    if options.live_reload && page.source.is_some() {
        html.push_str(
            "<section class=\"source-panel\" id=\"source-panel\" aria-label=\"Notist source\" hidden><div class=\"source-head\"><span class=\"source-title\">Notist source</span><button class=\"source-copy\" id=\"source-copy\" type=\"button\">Copy</button></div><pre class=\"source-code\" id=\"source-code\"></pre></section>\n",
        );
    }
    html.push_str(
        "<footer class=\"page-footer\"><span>Built with Notist</span><span class=\"page-module\">",
    );
    escape_html(&mut html, &page.module.to_string());
    html.push_str("</span></footer>\n</main>\n");
    html.push_str(&page_rail(page, options));
    html.push_str("\n</div>\n</div>\n<button class=\"icon-button to-top\" id=\"to-top\" type=\"button\" aria-label=\"Back to top\">");
    html.push_str(ICON_ARROW_UP);
    html.push_str("</button>\n</body>\n</html>\n");
    html
}

fn module_tree(current: &ModulePath, pages: &[PageView<'_>]) -> String {
    let mut output = String::from("<nav class=\"module-tree\" aria-label=\"Modules\"><ol>");
    if let Some(home) = pages.iter().find(|page| page.module.segments().is_empty()) {
        tree_link(&mut output, current, home);
    }
    for page in pages {
        if page.module.segments().len() == 1 {
            tree_item(&mut output, current, pages, page);
        }
    }
    output.push_str("</ol></nav>");
    output
}

fn tree_item(
    output: &mut String,
    current: &ModulePath,
    pages: &[PageView<'_>],
    page: &PageView<'_>,
) {
    tree_link(output, current, page);
    let segments = page.module.segments();
    let mut children = String::new();
    for child in pages {
        let child_segments = child.module.segments();
        if child_segments.len() == segments.len() + 1 && child_segments.starts_with(segments) {
            tree_item(&mut children, current, pages, child);
        }
    }
    if !children.is_empty() {
        output.push_str("<ol>");
        output.push_str(&children);
        output.push_str("</ol>");
    }
    output.push_str("</li>");
}

fn tree_link(output: &mut String, current: &ModulePath, page: &PageView<'_>) {
    output.push_str("<li><a href=\"");
    escape_attribute(output, &module_href(current, &page.module, None));
    if page.module == *current {
        output.push_str("\" aria-current=\"page");
    }
    output.push_str("\">");
    escape_html(output, page.label());
    output.push_str("</a>");
    if page.module.segments().is_empty() {
        output.push_str("</li>");
    }
}

fn breadcrumb(site_name: &str, page: &PageView<'_>) -> String {
    let segments = page.module.segments();
    if segments.is_empty() {
        return String::new();
    }
    let mut output =
        String::from("<nav class=\"breadcrumb\" aria-label=\"Breadcrumb\"><ol><li><a href=\"");
    escape_attribute(
        &mut output,
        &module_href(&page.module, &ModulePath::root(), None),
    );
    output.push_str("\">");
    escape_html(&mut output, site_name);
    output.push_str("</a></li>");
    for depth in 1..segments.len() {
        let ancestor = ModulePath::from_segments(segments[..depth].to_vec());
        output.push_str("<li><a href=\"");
        escape_attribute(&mut output, &module_href(&page.module, &ancestor, None));
        output.push_str("\">");
        escape_html(&mut output, &segments[depth - 1]);
        output.push_str("</a></li>");
    }
    output.push_str("<li aria-current=\"page\">");
    escape_html(&mut output, page.label());
    output.push_str("</li></ol></nav>\n");
    output
}

/// The right rail: the page TOC, plus — in preview only — a tab strip with a
/// "Symbols" tab that hosts the enhanced-mode inspector. Static builds keep
/// the plain TOC aside; preview always renders the rail so the inspector tab
/// has a home even on pages without headings.
fn page_rail(page: &PageView<'_>, options: SiteOptions) -> String {
    let has_toc = page
        .headings
        .iter()
        .any(|heading| (2..=5).contains(&heading.level));
    if !has_toc && !options.live_reload {
        return String::new();
    }
    let mut output = String::from("<aside class=\"page-rail\"");
    if !has_toc {
        output.push_str(" data-empty-toc");
    }
    output.push('>');
    if options.live_reload {
        output.push_str(
            "<div class=\"rail-tabs\" role=\"tablist\" aria-label=\"Page rail\"><button class=\"rail-tab\" id=\"rail-tab-toc\" type=\"button\" role=\"tab\" aria-selected=\"true\" aria-controls=\"rail-panel-toc\">On this page</button><button class=\"rail-tab\" id=\"rail-tab-inspector\" type=\"button\" role=\"tab\" aria-selected=\"false\" aria-controls=\"rail-panel-inspector\">Symbols</button></div>",
        );
        output.push_str(
            "<div class=\"page-toc\" id=\"rail-panel-toc\" role=\"tabpanel\" aria-labelledby=\"rail-tab-toc\">",
        );
    } else {
        output.push_str("<div class=\"page-toc\">");
    }
    output.push_str("<div class=\"toc-title\">On this page</div>");
    if has_toc {
        output.push_str("<ol>");
        for heading in page.headings {
            if !(2..=5).contains(&heading.level) {
                continue;
            }
            write!(
                output,
                "<li style=\"--toc-level:{}\"><a href=\"#",
                heading.level
            )
            .unwrap();
            escape_attribute(&mut output, &heading.id);
            output.push_str("\">");
            escape_html(&mut output, &heading.text);
            output.push_str("</a></li>");
        }
        output.push_str("</ol>");
    } else {
        output.push_str("<div class=\"toc-empty\">No headings</div>");
    }
    output.push_str("</div>");
    if options.live_reload {
        output.push_str(
            "<div class=\"inspector-panel\" id=\"rail-panel-inspector\" role=\"tabpanel\" aria-labelledby=\"rail-tab-inspector\" hidden></div>",
        );
    }
    output.push_str("</aside>");
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

fn clean_output_root(output: &Path) -> Result<(), Box<dyn Error>> {
    if output.parent().is_none()
        || output == std::env::current_dir()?.as_path()
        || std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .is_some_and(|home| output.as_os_str() == home)
    {
        return Err(format!("refusing to clean broad output path `{}`", output.display()).into());
    }
    fs::remove_dir_all(output)?;
    fs::create_dir_all(output)?;
    Ok(())
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

const ICON_MENU: &str = r#"<svg viewBox="0 0 20 20" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true"><path d="M3 5.5h14M3 10h14M3 14.5h14"/></svg>"#;

const ICON_SUN: &str = r#"<svg class="icon-sun" viewBox="0 0 20 20" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true"><circle cx="10" cy="10" r="3.6"/><path d="M10 2.2v2M10 15.8v2M2.2 10h2M15.8 10h2M4.5 4.5l1.4 1.4M14.1 14.1l1.4 1.4M15.5 4.5l-1.4 1.4M5.9 14.1l-1.4 1.4"/></svg>"#;

const ICON_MOON: &str = r#"<svg class="icon-moon" viewBox="0 0 20 20" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round" aria-hidden="true"><path d="M16.5 11.5A6.5 6.5 0 1 1 8.5 3.5 5.4 5.4 0 0 0 16.5 11.5Z"/></svg>"#;

const ICON_ARROW_UP: &str = r#"<svg viewBox="0 0 20 20" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M10 16V4M4.5 9.5 10 4l5.5 5.5"/></svg>"#;

/// Applies a persisted theme before first paint to avoid a light/dark flash.
const THEME_BOOTSTRAP: &str = r#"(()=>{try{const t=localStorage.getItem("notist-theme");if(t==="light"||t==="dark")document.documentElement.dataset.theme=t}catch(e){}})()"#;

const SITE_SCRIPT: &str = r#"(() => {
  "use strict";
  const doc = document.documentElement;

  // --- color theme toggle ---
  const systemDark = matchMedia("(prefers-color-scheme: dark)");
  const currentTheme = () =>
    doc.dataset.theme === "light" || doc.dataset.theme === "dark"
      ? doc.dataset.theme
      : systemDark.matches
        ? "dark"
        : "light";
  document.getElementById("theme-toggle")?.addEventListener("click", () => {
    const next = currentTheme() === "dark" ? "light" : "dark";
    doc.dataset.theme = next;
    try {
      localStorage.setItem("notist-theme", next);
    } catch {}
  });

  // --- navigation drawer (mobile) ---
  const navToggle = document.getElementById("nav-toggle");
  const setDrawer = (open) => {
    document.body.classList.toggle("nav-open", open);
    navToggle?.setAttribute("aria-expanded", open ? "true" : "false");
  };
  navToggle?.addEventListener("click", () =>
    setDrawer(!document.body.classList.contains("nav-open")),
  );
  document
    .querySelector(".sidebar-scrim")
    ?.addEventListener("click", () => setDrawer(false));
  addEventListener("keydown", (event) => {
    if (event.key === "Escape") setDrawer(false);
  });
  document
    .getElementById("site-sidebar")
    ?.addEventListener("click", (event) => {
      if (event.target instanceof Element && event.target.closest("a")) setDrawer(false);
    });

  // --- preserve sidebar scroll position across page navigations ---
  // The sidebar is its own scroll container (overflow-y: auto), so a full
  // page load would reset it to the top even though the browser restores the
  // main document scroll. The tree is identical on every page, so one shared
  // position is saved and restored for the whole session.
  const sidebar = document.getElementById("site-sidebar");
  if (sidebar) {
    const sidebarScrollKey = "notist-sidebar-scroll";
    const savedSidebarScroll = sessionStorage.getItem(sidebarScrollKey);
    if (savedSidebarScroll !== null) {
      sidebar.scrollTop = Number(savedSidebarScroll);
    }
    let sidebarTicking = false;
    sidebar.addEventListener(
      "scroll",
      () => {
        if (sidebarTicking) return;
        sidebarTicking = true;
        requestAnimationFrame(() => {
          sidebarTicking = false;
          try {
            sessionStorage.setItem(sidebarScrollKey, String(sidebar.scrollTop));
          } catch {}
        });
      },
      { passive: true },
    );
  }

  // --- scroll effects: table-of-contents spy + back to top ---
  const toTop = document.getElementById("to-top");
  const tocLinks = Array.from(document.querySelectorAll(".page-toc a[href^='#']"));
  const tocTargets = tocLinks
    .map((link) => {
      const element = document.getElementById(link.getAttribute("href").slice(1));
      return element ? { element, link } : null;
    })
    .filter((entry) => entry !== null);
  let activeTocLink = null;
  const updateScrollEffects = () => {
    toTop?.classList.toggle("visible", scrollY > 640);
    let next = null;
    for (const { element, link } of tocTargets) {
      if (element.getBoundingClientRect().top <= 96) next = link;
      else break;
    }
    if (next !== activeTocLink) {
      activeTocLink?.removeAttribute("aria-current");
      next?.setAttribute("aria-current", "true");
      activeTocLink = next;
    }
  };
  let ticking = false;
  addEventListener(
    "scroll",
    () => {
      if (!ticking) {
        ticking = true;
        requestAnimationFrame(() => {
          updateScrollEffects();
          ticking = false;
        });
      }
    },
    { passive: true },
  );
  toTop?.addEventListener("click", () =>
    scrollTo({
      top: 0,
      behavior: matchMedia("(prefers-reduced-motion: reduce)").matches
        ? "auto"
        : "smooth",
    }),
  );
  updateScrollEffects();
})();
"#;

const LIVE_RELOAD_SCRIPT: &str = r#"(() => {
  "use strict";
  const eventsUrl = new URL("events", document.currentScript.src);

  const pill = document.createElement("div");
  pill.className = "live-status";
  pill.setAttribute("role", "status");
  const dot = document.createElement("span");
  dot.className = "live-dot";
  const label = document.createElement("span");
  pill.append(dot, label);
  const setState = (state, text) => {
    pill.dataset.state = state;
    label.textContent = text;
  };
  setState("down", "Connecting…");
  // The pill lives in the top-right preview toolbar next to the enhanced-mode
  // switch; the body fallback only matters for pages without the chrome.
  (document.querySelector(".preview-chrome") ?? document.body).append(pill);

  // Restore the reading position after a rebuild-triggered reload.
  const scrollKey = `notist-scroll:${location.pathname}`;
  const savedScroll = sessionStorage.getItem(scrollKey);
  if (savedScroll !== null) {
    sessionStorage.removeItem(scrollKey);
    addEventListener("load", () => scrollTo({ top: Number(savedScroll) }), { once: true });
  }

  let revision;
  let events;

  const openEvents = () => {
    if (events && events.readyState !== EventSource.CLOSED) {
      return;
    }
    setState("down", "Connecting…");
    events = new EventSource(eventsUrl);
    events.onopen = () => setState("live", "Live reload");
    events.onerror = () => setState("down", "Reconnecting…");
    events.onmessage = (event) => {
      if (revision !== undefined && revision !== event.data) {
        sessionStorage.setItem(scrollKey, String(Math.round(scrollY)));
        setState("sync", "Updating…");
        location.reload();
        return;
      }
      revision = event.data;
    };
  };

  const closeEvents = () => {
    if (!events) return;
    events.close();
    events = null;
  };

  // A hidden or back/forward-cached page must not keep holding an EventSource
  // connection: browsers cap connections per host, and rapid page navigation
  // can otherwise exhaust every socket and stall the next visible page load.
  addEventListener("pagehide", closeEvents);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") closeEvents();
    else openEvents();
  });
  addEventListener("pageshow", (event) => {
    if (event.persisted) openEvents();
  });

  if (document.visibilityState !== "hidden") openEvents();
})();
"#;

/// Enhanced mode: a client-side document inspector. The toggle switch lives in
/// the top-right preview toolbar; when on, annotated regions are outlined, the
/// right rail gains a "Symbols" tab listing the page's anchors and annotation
/// regions, and hovering any rendered element shows its source range, anchor,
/// tags, and annotation properties. All data comes from the `data-notist-*`
/// attributes the renderer already emits, so rebuilds need no extra protocol
/// round-trips.
const INSPECT_SCRIPT: &str = r#"(() => {
  "use strict";
  const toggle = document.getElementById("inspect-toggle");
  const article = document.querySelector(".notist-document");
  const tocTab = document.getElementById("rail-tab-toc");
  const inspectorTab = document.getElementById("rail-tab-inspector");
  const tocPanel = document.getElementById("rail-panel-toc");
  const inspectorPanel = document.getElementById("rail-panel-inspector");
  if (!toggle || !article || !inspectorPanel) return;

  const STORAGE_KEY = "notist-enhanced";
  // Renderer-reserved dataset keys; every other data-notist-* attribute is a
  // user-defined annotation property.
  const RESERVED = new Set([
    "notistStart",
    "notistEnd",
    "notistTag",
    "notistKind",
    "notistName",
    "notistArguments",
    "notistColumns",
  ]);

  let tooltip = null;
  let hoverElement = null;
  let hoverInner = null;
  let moveTicking = false;
  let lastX = 0;
  let lastY = 0;

  const enhanced = () => document.body.classList.contains("enhanced");

  const setEnhanced = (on) => {
    document.body.classList.toggle("enhanced", on);
    toggle.setAttribute("aria-checked", on ? "true" : "false");
    try {
      sessionStorage.setItem(STORAGE_KEY, on ? "1" : "0");
    } catch {}
    if (on) openPanel();
    else teardown();
  };

  toggle.addEventListener("click", () => setEnhanced(!enhanced()));

  // --- rail tabs ---
  const switchTab = (which) => {
    const showInspector = which === "inspector";
    tocTab?.setAttribute("aria-selected", showInspector ? "false" : "true");
    inspectorTab?.setAttribute("aria-selected", showInspector ? "true" : "false");
    if (tocPanel) tocPanel.hidden = showInspector;
    inspectorPanel.hidden = !showInspector;
  };
  tocTab?.addEventListener("click", () => switchTab("toc"));
  inspectorTab?.addEventListener("click", () => switchTab("inspector"));

  // --- data extraction ---
  const propertiesOf = (element) =>
    Object.entries(element.dataset).filter(([key]) => !RESERVED.has(key));

  const tagsOf = (element) =>
    (element.dataset.notistTag || "").split(/\s+/).filter(Boolean);

  const isAnnotated = (element) =>
    element.classList.contains("notist-annotated") ||
    element.hasAttribute("data-notist-tag") ||
    propertiesOf(element).length > 0;

  // An element worth inspecting on its own: carries an anchor or annotation
  // attributes, not just a source range.
  const isMeaningful = (element) => !!element.id || isAnnotated(element);

  const sameRange = (a, b) =>
    a.hasAttribute("data-notist-start") &&
    b.hasAttribute("data-notist-start") &&
    a.dataset.notistStart === b.dataset.notistStart &&
    a.dataset.notistEnd === b.dataset.notistEnd;

  const tightlyWraps = (parent, child) =>
    parent.children.length === 1 && parent.firstElementChild === child;

  // Resolves a hovered innermost ranged element to the scope the user means:
  // the nearest meaningful ancestor-or-self, continuing outward only through
  // tightly-coincident elements — same-range projections, and single-child
  // chains involving a rangeless annotation wrapper (the heading carrying the
  // id around its wrapper). Nested scopes stop at the inner one: an annotated
  // wrapper with siblings is a scope of its own, not part of the parent.
  const resolveHover = (inner) => {
    let current = inner;
    while (current && current !== article) {
      if (isMeaningful(current)) {
        let resolved = current;
        let next = current.parentElement;
        while (
          next &&
          next !== article &&
          isMeaningful(next) &&
          (sameRange(current, next) ||
            (tightlyWraps(next, current) &&
              (!next.hasAttribute("data-notist-start") ||
                !current.hasAttribute("data-notist-start"))))
        ) {
          current = next;
          resolved = next;
          next = next.parentElement;
        }
        return resolved;
      }
      current = current.parentElement;
    }
    return inner;
  };

  // Merges the id, range, tags, and properties scattered across the chain
  // from the innermost element up to the resolved scope: the renderer splits
  // them (id on the heading, tags on its wrapper), but the scope is one unit.
  const collectInfo = (inner, resolved) => {
    let id = null;
    let range = null;
    const tags = [];
    const properties = new Map();
    let current = inner;
    while (current) {
      if (!id && current.id) id = current.id;
      if (current.hasAttribute("data-notist-start")) range = rangeOf(current);
      for (const tag of tagsOf(current)) {
        if (!tags.includes(tag)) tags.push(tag);
      }
      for (const [key, value] of propertiesOf(current)) {
        if (!properties.has(key)) properties.set(key, value);
      }
      if (current === resolved) break;
      current = current.parentElement;
    }
    return { id, range, tags, properties };
  };

  const kindOf = (element) => element.tagName.toLowerCase();

  const rangeOf = (element) =>
    element.dataset.notistStart !== undefined
      ? `${element.dataset.notistStart}–${element.dataset.notistEnd}`
      : null;

  const propertyName = (key) =>
    key.replace(/^notist/, "").replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);

  const span = (className, text) => {
    const node = document.createElement("span");
    node.className = className;
    node.textContent = text;
    return node;
  };

  // The module's root bindings, embedded by the page shell as JSON.
  const pageBindings = (() => {
    const node = document.getElementById("notist-bindings");
    if (!node) return [];
    try {
      const parsed = JSON.parse(node.textContent);
      return Array.isArray(parsed) ? parsed : [];
    } catch {
      return [];
    }
  })();

  // --- inspector panel ---
  const section = (title, empty, items) => {
    const block = document.createElement("section");
    block.className = "inspector-section";
    const heading = document.createElement("div");
    heading.className = "inspector-heading";
    heading.textContent = `${title} (${items.length})`;
    block.append(heading);
    if (items.length === 0) {
      block.append(span("inspector-empty", empty));
      return block;
    }
    const list = document.createElement("ol");
    for (const item of items) {
      const entry = document.createElement("li");
      // Items with a DOM target are jump buttons; informational items
      // (bindings produce no rendered element) are static rows.
      const row = document.createElement(item.element ? "button" : "div");
      row.className = item.element
        ? "inspector-item"
        : "inspector-item inspector-static";
      if (item.element) {
        row.type = "button";
        row.addEventListener("click", () => jumpTo(item.element));
      }
      row.append(...item.nodes);
      entry.append(row);
      list.append(entry);
    }
    block.append(list);
    return block;
  };

  const symbolItems = () =>
    Array.from(article.querySelectorAll("[id]")).map((element) => ({
      element,
      nodes: [span("inspector-id", `#${element.id}`), span("inspector-kind", kindOf(element))],
    }));

  const bindingItems = () =>
    pageBindings.map((binding) => ({
      element: null,
      nodes: [
        span("inspector-id", binding.name),
        span("inspector-kind", binding.detail),
      ],
    }));

  const annotationItems = () =>
    Array.from(article.querySelectorAll("[data-notist-start]"))
      .filter(isAnnotated)
      .map((element) => {
        const tags = tagsOf(element);
        const properties = propertiesOf(element);
        const label =
          tags.length > 0
            ? tags.map((tag) => `@${tag}`).join(" ")
            : properties.length > 0
              ? properties.map(([key]) => propertyName(key)).join(" ")
              : "annotation";
        const nodes = [span("inspector-id", label), span("inspector-kind", kindOf(element))];
        const range = rangeOf(element);
        if (range) nodes.push(span("inspector-range", range));
        return { element, nodes };
      });

  const openPanel = () => {
    inspectorPanel.replaceChildren(
      section("Anchors", "No anchors on this page", symbolItems()),
      section("Bindings", "No bindings in this module", bindingItems()),
      section("Annotations", "No annotations on this page", annotationItems()),
    );
    switchTab("inspector");
  };

  const teardown = () => {
    inspectorPanel.replaceChildren();
    switchTab("toc");
    hideTooltip();
    clearHover();
  };

  const jumpTo = (element) => {
    element.scrollIntoView({
      block: "center",
      behavior: matchMedia("(prefers-reduced-motion: reduce)").matches
        ? "auto"
        : "smooth",
    });
    // Restart the flash animation even for repeated jumps to the same element.
    element.classList.remove("inspect-flash");
    void element.offsetWidth;
    element.classList.add("inspect-flash");
  };

  // --- hover tooltip ---
  const describe = (inner, resolved) => {
    const info = collectInfo(inner, resolved);
    const rows = [];
    const head = document.createElement("div");
    head.className = "tt-head";
    head.append(span("tt-kind", kindOf(resolved)));
    if (info.id) head.append(span("tt-id", `#${info.id}`));
    if (info.range) head.append(span("tt-range", `[${info.range})`));
    rows.push(head);
    for (const tag of info.tags) {
      const row = document.createElement("div");
      row.append(span("tt-key", "tag"), document.createTextNode(` ${tag}`));
      rows.push(row);
    }
    for (const [key, value] of info.properties) {
      const row = document.createElement("div");
      row.append(
        span("tt-key", propertyName(key)),
        document.createTextNode(` = ${value}`),
      );
      rows.push(row);
    }
    return rows;
  };

  const showTooltip = (x, y) => {
    if (!tooltip) {
      tooltip = document.createElement("div");
      tooltip.className = "inspect-tooltip";
      tooltip.setAttribute("aria-hidden", "true");
      document.body.append(tooltip);
    }
    tooltip.replaceChildren(...describe(hoverInner, hoverElement));
    tooltip.style.left = "0px";
    tooltip.style.top = "0px";
    const width = tooltip.offsetWidth;
    const height = tooltip.offsetHeight;
    let left = x + 14;
    let top = y + 16;
    if (left + width > innerWidth - 8) left = x - width - 14;
    if (top + height > innerHeight - 8) top = y - height - 16;
    tooltip.style.left = `${Math.max(8, left)}px`;
    tooltip.style.top = `${Math.max(8, top)}px`;
  };

  const hideTooltip = () => {
    tooltip?.remove();
    tooltip = null;
  };

  const clearHover = () => {
    hoverElement?.classList.remove("inspect-hover");
    hoverElement = null;
    hoverInner = null;
  };

  article.addEventListener("mouseover", (event) => {
    if (!enhanced()) return;
    const inner =
      event.target instanceof Element
        ? event.target.closest("[data-notist-start]")
        : null;
    const resolved = inner ? resolveHover(inner) : null;
    if (resolved === hoverElement) return;
    clearHover();
    hoverElement = resolved;
    hoverInner = inner;
    hoverElement?.classList.add("inspect-hover");
    if (!hoverElement) hideTooltip();
  });

  article.addEventListener("mousemove", (event) => {
    if (!enhanced()) return;
    lastX = event.clientX;
    lastY = event.clientY;
    if (moveTicking) return;
    moveTicking = true;
    requestAnimationFrame(() => {
      moveTicking = false;
      if (hoverElement && enhanced()) showTooltip(lastX, lastY);
    });
  });

  article.addEventListener("mouseleave", () => {
    clearHover();
    hideTooltip();
  });
  addEventListener("scroll", hideTooltip, { capture: true, passive: true });

  // Persisted across the reloads that live rebuilds trigger.
  let saved = null;
  try {
    saved = sessionStorage.getItem(STORAGE_KEY);
  } catch {}
  if (saved === "1") setEnhanced(true);
})();
"#;

/// Preview source toggle: switches the main column between the rendered
/// document and the raw `.not` source embedded by the page shell. The source
/// travels with the page (from the same snapshot as the fragment), so this
/// needs no extra round-trip to the service.
const SOURCE_SCRIPT: &str = r#"(() => {
  "use strict";
  const toggle = document.getElementById("source-toggle");
  const article = document.querySelector(".notist-document");
  const panel = document.getElementById("source-panel");
  const code = document.getElementById("source-code");
  if (!toggle || !article || !panel || !code) return;

  const STORAGE_KEY = "notist-source";
  const sourceNode = document.getElementById("notist-source");
  let source = null;
  if (sourceNode) {
    try {
      const parsed = JSON.parse(sourceNode.textContent);
      source = typeof parsed === "string" ? parsed : null;
    } catch {}
  }
  if (source === null) {
    toggle.disabled = true;
    toggle.title = "This virtual module has no .not source file";
    return;
  }

  // One block per source line. CSS paints the line number in a hanging
  // gutter, so long lines wrap with the same indentation as their first
  // line. The copy button always copies the exact `source` text; only the
  // display normalizes CRLF/CR line endings into LF.
  const normalized = source.replace(/\r\n?/g, "\n");
  const lines = normalized.split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  const fragment = document.createDocumentFragment();
  for (const text of lines) {
    const line = document.createElement("span");
    line.className = "source-line";
    line.textContent = text;
    fragment.append(line);
  }
  code.replaceChildren(fragment);

  const sourceOpen = () => document.body.classList.contains("source-open");

  const setSource = (on) => {
    document.body.classList.toggle("source-open", on);
    article.hidden = on;
    panel.hidden = !on;
    toggle.setAttribute("aria-checked", on ? "true" : "false");
    try {
      sessionStorage.setItem(STORAGE_KEY, on ? "1" : "0");
    } catch {}
    if (on) {
      // The two views have different heights; start reading from the top.
      document.getElementById("page-content")?.scrollIntoView();
    }
  };

  toggle.addEventListener("click", () => setSource(!sourceOpen()));

  const copy = document.getElementById("source-copy");
  copy?.addEventListener("click", async () => {
    const reset = () => {
      copy.textContent = "Copy";
      copy.classList.remove("copied");
    };
    try {
      await navigator.clipboard.writeText(source);
      copy.textContent = "Copied";
      copy.classList.add("copied");
      setTimeout(reset, 1600);
    } catch {
      copy.textContent = "Copy failed";
      setTimeout(reset, 1600);
    }
  });

  // Persisted across the reloads that live rebuilds trigger.
  let saved = null;
  try {
    saved = sessionStorage.getItem(STORAGE_KEY);
  } catch {}
  if (saved === "1") setSource(true);
})();
"#;

const STYLES: &str = r#"/* Notist site chrome + document styles (build & preview share this file). */

/* ---------- design tokens ---------- */
:root {
  color-scheme: light;
  --font-sans: "Inter", ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto,
    "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", "Noto Sans CJK SC", sans-serif;
  --font-mono: ui-monospace, "SF Mono", "Cascadia Code", "JetBrains Mono", Menlo,
    Consolas, monospace;
  --bg: #f4f5f7;
  --surface: #ffffff;
  --sunken: #eceef1;
  --text: #23262d;
  --text-strong: #12141a;
  --muted: #5f6b7a;
  --faint: #98a2b3;
  --border: #d9dee5;
  --border-soft: #e7eaef;
  --accent: #4f46e5;
  --accent-strong: #3f3acb;
  --accent-soft: color-mix(in srgb, var(--accent) 9%, transparent);
  --info: #2563eb;
  --success: #0a8a5f;
  --warning: #b45309;
  --danger: #c62828;
  --mark: rgb(250 204 21 / 0.45);
  --shadow-sm: 0 1px 2px rgb(16 24 40 / 0.06);
  --shadow-md: 0 6px 16px rgb(16 24 40 / 0.1);
  --radius: 10px;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    color-scheme: dark;
    --bg: #0e1116;
    --surface: #161a21;
    --sunken: #10141b;
    --text: #d3d9e2;
    --text-strong: #eceff5;
    --muted: #8b95a5;
    --faint: #5d6675;
    --border: #272e3a;
    --border-soft: #1e242e;
    --accent: #8f93f8;
    --accent-strong: #a9adff;
    --accent-soft: color-mix(in srgb, var(--accent) 15%, transparent);
    --info: #6ea8fe;
    --success: #34d399;
    --warning: #f5b04c;
    --danger: #f08a80;
    --mark: rgb(250 204 21 / 0.3);
    --shadow-sm: 0 1px 2px rgb(0 0 0 / 0.4);
    --shadow-md: 0 6px 16px rgb(0 0 0 / 0.5);
  }
}
:root[data-theme="dark"] {
  color-scheme: dark;
  --bg: #0e1116;
  --surface: #161a21;
  --sunken: #10141b;
  --text: #d3d9e2;
  --text-strong: #eceff5;
  --muted: #8b95a5;
  --faint: #5d6675;
  --border: #272e3a;
  --border-soft: #1e242e;
  --accent: #8f93f8;
  --accent-strong: #a9adff;
  --accent-soft: color-mix(in srgb, var(--accent) 15%, transparent);
  --info: #6ea8fe;
  --success: #34d399;
  --warning: #f5b04c;
  --danger: #f08a80;
  --mark: rgb(250 204 21 / 0.3);
  --shadow-sm: 0 1px 2px rgb(0 0 0 / 0.4);
  --shadow-md: 0 6px 16px rgb(0 0 0 / 0.5);
}

/* ---------- base ---------- */
* { box-sizing: border-box; }
html { scroll-behavior: smooth; scroll-padding-top: 28px; }
body {
  margin: 0;
  background: var(--bg);
  color: var(--text);
  font: 16px/1.75 var(--font-sans);
  -webkit-font-smoothing: antialiased;
  text-rendering: optimizeLegibility;
}
::selection { background: color-mix(in srgb, var(--accent) 22%, transparent); }
:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; border-radius: 4px; }
a {
  color: var(--accent-strong);
  text-decoration: underline;
  text-decoration-color: color-mix(in srgb, var(--accent) 35%, transparent);
  text-decoration-thickness: 1px;
  text-underline-offset: 3px;
}
a:hover { text-decoration-color: currentColor; }
button { font: inherit; }

.skip-link {
  position: fixed;
  top: -100px;
  left: 16px;
  z-index: 100;
  padding: 8px 14px;
  border-radius: 8px;
  background: var(--accent);
  color: #fff;
  text-decoration: none;
  transition: top 0.15s ease;
}
.skip-link:focus-visible { top: 12px; }

/* ---------- layout ---------- */
.site-layout { display: flex; min-height: 100vh; min-height: 100dvh; }
.site-sidebar {
  flex: none;
  width: 300px;
  position: sticky;
  top: 0;
  height: 100vh;
  height: 100dvh;
  overflow-y: auto;
  padding: 20px 14px 20px 20px;
  background: var(--surface);
  border-right: 1px solid var(--border-soft);
}
.page-body {
  flex: 1;
  min-width: 0;
  display: flex;
  justify-content: center;
  gap: 56px;
  padding: 44px 48px 96px;
}
.page-main { width: min(100%, 46rem); }
.page-rail {
  flex: none;
  width: 15rem;
  position: sticky;
  top: 44px;
  align-self: flex-start;
  max-height: calc(100vh - 88px);
  max-height: calc(100dvh - 88px);
  overflow-y: auto;
  padding-bottom: 24px;
}
/* Preview pages without headings hide the rail until enhanced mode needs it. */
.page-rail[data-empty-toc] { display: none; }
body.enhanced .page-rail[data-empty-toc] { display: block; }
@media (max-width: 1240px) {
  .page-rail { display: none; }
  .page-body { gap: 0; }
}

/* rail tabs (enhanced mode only; the strip is hidden otherwise) */
.rail-tabs { display: flex; gap: 4px; margin-bottom: 14px; }
body:not(.enhanced) .rail-tabs { display: none; }
body.enhanced .toc-title { display: none; }
.rail-tab {
  flex: 1;
  padding: 5px 8px;
  border: 1px solid var(--border-soft);
  border-radius: 7px;
  background: transparent;
  color: var(--muted);
  font-size: 12px;
  line-height: 1.4;
  cursor: pointer;
  transition: background-color 0.12s ease, color 0.12s ease;
}
.rail-tab:hover { background: var(--sunken); color: var(--text); }
.rail-tab[aria-selected="true"] {
  border-color: transparent;
  background: var(--accent-soft);
  color: var(--accent-strong);
  font-weight: 600;
}
.toc-empty { padding-left: 14px; color: var(--faint); font-size: 12.5px; }

/* ---------- sidebar ---------- */
.sidebar-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 16px;
  padding: 2px 6px;
}
.site-name {
  color: var(--text-strong);
  font-size: 15px;
  font-weight: 700;
  letter-spacing: 0.01em;
  text-decoration: none;
}
.site-name:hover { color: var(--accent-strong); }
.module-tree ol { margin: 0; padding: 0; list-style: none; }
.module-tree li { margin: 1px 0; }
.module-tree li > ol {
  margin: 2px 0 4px 11px;
  padding-left: 10px;
  border-left: 1px solid var(--border-soft);
}
.module-tree a {
  display: block;
  padding: 5px 10px;
  border-radius: 7px;
  color: var(--muted);
  font-size: 13.5px;
  line-height: 1.55;
  text-decoration: none;
  overflow-wrap: anywhere;
  transition: background-color 0.12s ease, color 0.12s ease;
}
.module-tree a:hover { background: var(--sunken); color: var(--text); }
.module-tree a[aria-current="page"] {
  background: var(--accent-soft);
  color: var(--accent-strong);
  font-weight: 600;
}

/* ---------- theme toggle ---------- */
.icon-button {
  display: inline-grid;
  place-items: center;
  width: 32px;
  height: 32px;
  padding: 0;
  border: 1px solid transparent;
  border-radius: 8px;
  background: transparent;
  color: var(--muted);
  cursor: pointer;
  transition: background-color 0.12s ease, color 0.12s ease, border-color 0.12s ease;
}
.icon-button:hover { background: var(--sunken); color: var(--text); }
.theme-toggle .icon-sun { display: none; }
:root[data-theme="dark"] .theme-toggle .icon-sun { display: block; }
:root[data-theme="dark"] .theme-toggle .icon-moon { display: none; }
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) .theme-toggle .icon-sun { display: block; }
  :root:not([data-theme="light"]) .theme-toggle .icon-moon { display: none; }
}

/* ---------- table of contents rail ---------- */
.toc-title {
  margin-bottom: 10px;
  padding-left: 14px;
  color: var(--faint);
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.12em;
  text-transform: uppercase;
}
.page-toc ol {
  margin: 0;
  padding: 0;
  list-style: none;
  border-left: 1px solid var(--border-soft);
}
.page-toc a {
  display: block;
  margin-left: -1px;
  padding: 4px 0 4px calc(13px + (var(--toc-level) - 2) * 14px);
  border-left: 2px solid transparent;
  color: var(--muted);
  font-size: 13px;
  line-height: 1.5;
  text-decoration: none;
  overflow-wrap: anywhere;
}
.page-toc a:hover { color: var(--text); }
.page-toc a[aria-current="true"] {
  border-left-color: var(--accent);
  color: var(--accent-strong);
  font-weight: 550;
}

/* ---------- breadcrumb ---------- */
.breadcrumb { margin-bottom: 30px; font-size: 13px; }
.breadcrumb ol {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 2px;
  margin: 0;
  padding: 0;
  list-style: none;
}
.breadcrumb li { display: flex; align-items: center; min-width: 0; }
.breadcrumb li + li::before { content: "/"; margin: 0 9px; color: var(--faint); }
.breadcrumb a {
  padding: 2px 4px;
  border-radius: 5px;
  color: var(--muted);
  text-decoration: none;
}
.breadcrumb a:hover { color: var(--accent-strong); background: var(--accent-soft); }
.breadcrumb [aria-current="page"] {
  padding: 2px 4px;
  color: var(--text);
  font-weight: 550;
}

/* ---------- page footer ---------- */
.page-footer {
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  gap: 8px 16px;
  margin-top: 72px;
  padding-top: 18px;
  border-top: 1px solid var(--border-soft);
  color: var(--faint);
  font-size: 12.5px;
}
.page-module { font-family: var(--font-mono); font-size: 12px; }

/* ---------- floating controls ---------- */
.to-top {
  position: fixed;
  right: 24px;
  bottom: 24px;
  z-index: 40;
  width: 40px;
  height: 40px;
  border: 1px solid var(--border);
  border-radius: 12px;
  background: var(--surface);
  box-shadow: var(--shadow-sm);
  opacity: 0;
  translate: 0 8px;
  pointer-events: none;
  transition: opacity 0.2s ease, translate 0.2s ease;
}
.to-top.visible { opacity: 1; translate: 0; pointer-events: auto; }
.to-top:hover { border-color: var(--accent); color: var(--accent-strong); }

/* ---------- preview chrome (preview only) ---------- */
.preview-chrome {
  position: fixed;
  top: 14px;
  right: 20px;
  z-index: 80;
  display: flex;
  align-items: center;
  gap: 10px;
}

.live-status {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 13px;
  border: 1px solid var(--border);
  border-radius: 999px;
  background: var(--surface);
  box-shadow: var(--shadow-sm);
  color: var(--muted);
  font-size: 12px;
  line-height: 1;
}
.live-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--faint); }
.live-status[data-state="live"] .live-dot {
  background: #10b981;
  box-shadow: 0 0 0 3px color-mix(in srgb, #10b981 20%, transparent);
}
.live-status[data-state="sync"] .live-dot { background: var(--info); }
.live-status[data-state="down"] .live-dot {
  background: var(--warning);
  animation: live-pulse 1s ease-in-out infinite;
}
@keyframes live-pulse { 50% { opacity: 0.3; } }

/* preview toolbar switches (source view + enhanced mode) */
.chrome-toggle {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 6px 12px;
  border: 1px solid var(--border);
  border-radius: 999px;
  background: var(--surface);
  box-shadow: var(--shadow-sm);
  color: var(--muted);
  font-size: 12px;
  line-height: 1;
  cursor: pointer;
  transition: color 0.12s ease, border-color 0.12s ease, background-color 0.12s ease;
}
.chrome-toggle:hover { color: var(--text); border-color: var(--accent); }
.chrome-toggle[aria-checked="true"] {
  border-color: var(--accent);
  background: var(--accent-soft);
  color: var(--accent-strong);
  font-weight: 600;
}
.chrome-toggle:disabled {
  opacity: 0.55;
  cursor: not-allowed;
  box-shadow: none;
}
.chrome-switch {
  position: relative;
  flex: none;
  width: 26px;
  height: 15px;
  border-radius: 999px;
  background: var(--border);
  transition: background-color 0.15s ease;
}
.chrome-switch::after {
  content: "";
  position: absolute;
  top: 2px;
  left: 2px;
  width: 11px;
  height: 11px;
  border-radius: 50%;
  background: var(--surface);
  box-shadow: var(--shadow-sm);
  transition: translate 0.15s ease;
}
.chrome-toggle[aria-checked="true"] .chrome-switch { background: var(--accent); }
.chrome-toggle[aria-checked="true"] .chrome-switch::after { translate: 11px 0; }

/* source view (preview only) */
.source-panel {
  overflow: hidden;
  border: 1px solid var(--border-soft);
  border-radius: var(--radius);
  background: var(--surface);
}
.source-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 9px 14px;
  border-bottom: 1px solid var(--border-soft);
  background: var(--sunken);
}
.source-title {
  color: var(--muted);
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.12em;
  text-transform: uppercase;
}
.source-copy {
  padding: 4px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--surface);
  color: var(--muted);
  font-size: 12px;
  line-height: 1.4;
  cursor: pointer;
  transition: color 0.12s ease, border-color 0.12s ease;
}
.source-copy:hover { color: var(--text); border-color: var(--accent); }
.source-copy.copied { color: var(--success); border-color: var(--success); }
.source-code {
  counter-reset: source-line;
  margin: 0;
  padding: 16px 18px;
  overflow-x: auto;
  color: var(--text);
  font-family: var(--font-mono);
  font-size: 13.5px;
  line-height: 1.7;
}
.source-line {
  counter-increment: source-line;
  display: block;
  padding-left: 4.5ch;
  text-indent: -4.5ch;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.source-line::before {
  content: counter(source-line);
  display: inline-block;
  min-width: 3.5ch;
  margin-right: 1ch;
  color: var(--faint);
  text-align: right;
  user-select: none;
}
body.source-open .page-rail,
body.source-open.enhanced .page-rail[data-empty-toc] { display: none; }

/* enhanced-mode document highlights */
body.enhanced .notist-document [data-notist-tag],
body.enhanced .notist-document .notist-annotated {
  outline: 1px dashed color-mix(in srgb, var(--accent) 60%, transparent);
  outline-offset: 2px;
  border-radius: 3px;
}
/* The hover marker must beat the dashed annotation outline above, so it gets
   one more class-level selector than that rule. */
body.enhanced .notist-document [data-notist-start].inspect-hover,
body.enhanced .notist-document .notist-annotated.inspect-hover {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
  border-radius: 3px;
  background-color: color-mix(in srgb, var(--accent) 7%, transparent);
}
@keyframes inspect-flash {
  0%, 100% { background-color: transparent; }
  25% { background-color: color-mix(in srgb, var(--accent) 18%, transparent); }
}
.inspect-flash { animation: inspect-flash 0.9s ease; border-radius: 4px; }

/* inspector panel (a rail tab panel in preview) */
.inspector-panel { font-size: 12.5px; }
.inspector-heading {
  margin: 12px 0 6px;
  padding-left: 14px;
  color: var(--faint);
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.12em;
  text-transform: uppercase;
}
.inspector-section:first-child .inspector-heading { margin-top: 0; }
.inspector-panel ol { margin: 0; padding: 0; list-style: none; }
.inspector-item {
  display: flex;
  align-items: baseline;
  gap: 8px;
  width: 100%;
  padding: 5px 8px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--text);
  font-size: 12px;
  line-height: 1.5;
  text-align: left;
  cursor: pointer;
}
.inspector-item:hover { background: var(--sunken); }
.inspector-item.inspector-static { cursor: default; }
.inspector-item.inspector-static:hover { background: transparent; }
.inspector-id {
  color: var(--accent-strong);
  font-family: var(--font-mono);
  font-size: 11.5px;
  overflow-wrap: anywhere;
}
.inspector-kind { flex: none; color: var(--faint); font-size: 11px; }
.inspector-range {
  flex: none;
  margin-left: auto;
  color: var(--faint);
  font-family: var(--font-mono);
  font-size: 11px;
}
.inspector-empty { padding: 2px 8px 6px; color: var(--faint); font-size: 12px; }

/* hover tooltip */
.inspect-tooltip {
  position: fixed;
  z-index: 90;
  max-width: 340px;
  padding: 8px 11px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--surface);
  box-shadow: var(--shadow-md);
  color: var(--text);
  font: 11.5px/1.65 var(--font-mono);
  overflow-wrap: anywhere;
  pointer-events: none;
}
.tt-head { display: flex; flex-wrap: wrap; align-items: baseline; gap: 8px; }
.tt-kind { color: var(--accent-strong); font-weight: 700; }
.tt-id { color: var(--text-strong); }
.tt-range { color: var(--faint); }
.tt-key { color: var(--muted); }

/* ---------- topbar & drawer (mobile) ---------- */
.topbar { display: none; }
.sidebar-scrim { display: none; }
@media (max-width: 920px) {
  html { scroll-padding-top: 72px; }
  body { padding-top: 56px; }
  .topbar {
    display: flex;
    align-items: center;
    gap: 12px;
    position: fixed;
    inset: 0 0 auto 0;
    z-index: 50;
    height: 56px;
    padding: 0 14px;
    border-bottom: 1px solid var(--border-soft);
    background: color-mix(in srgb, var(--surface) 86%, transparent);
    -webkit-backdrop-filter: blur(12px);
    backdrop-filter: blur(12px);
  }
  .topbar-site {
    color: var(--text-strong);
    font-size: 14.5px;
    font-weight: 700;
    text-decoration: none;
  }
  .site-sidebar {
    position: fixed;
    top: 0;
    bottom: 0;
    left: 0;
    z-index: 60;
    width: min(320px, 86vw);
    height: auto;
    translate: -105% 0;
    transition: translate 0.25s ease;
  }
  body.nav-open .site-sidebar { translate: 0 0; box-shadow: var(--shadow-md); }
  .sidebar-scrim {
    display: block;
    position: fixed;
    inset: 0;
    z-index: 55;
    background: rgb(10 12 16 / 0.45);
    opacity: 0;
    pointer-events: none;
    transition: opacity 0.2s ease;
  }
  body.nav-open .sidebar-scrim { opacity: 1; pointer-events: auto; }
  .page-body { padding: 30px 20px 72px; }
  .preview-chrome { top: 64px; right: 12px; }
}

/* ---------- document typography ---------- */
.notist-document { overflow-wrap: break-word; }
.notist-document h1,
.notist-document h2,
.notist-document h3,
.notist-document h4,
.notist-document h5,
.notist-document h6 {
  margin: 1.9em 0 0.6em;
  color: var(--text-strong);
  font-weight: 650;
  letter-spacing: -0.01em;
  line-height: 1.35;
}
.notist-document h1 {
  margin-top: 0;
  padding-bottom: 0.35em;
  border-bottom: 1px solid var(--border-soft);
  font-size: 1.95rem;
}
.notist-document h2 { font-size: 1.5rem; margin-top: 2.1em; }
.notist-document h3 { font-size: 1.22rem; }
.notist-document h4 { font-size: 1.06rem; }
.notist-document h5, .notist-document h6 { font-size: 0.95rem; color: var(--muted); }
.notist-document p,
.notist-document ul,
.notist-document ol,
.notist-document dl,
.notist-document blockquote,
.notist-document pre,
.notist-document .notist-figure { margin: 0 0 1.1em; }
.notist-document ul, .notist-document ol { padding-left: 1.5em; }
.notist-document li + li { margin-top: 0.3em; }
.notist-document dt { font-weight: 650; }
.notist-document dd { margin: 0 0 0.75em 1.5em; }
.notist-document mark {
  padding: 0.05em 0.2em;
  border-radius: 4px;
  background: var(--mark);
  color: inherit;
}
.notist-document abbr {
  text-decoration: underline dotted;
  text-underline-offset: 0.18em;
  cursor: help;
}

/* inline code & code blocks */
.notist-document code {
  padding: 0.14em 0.38em;
  border: 1px solid var(--border-soft);
  border-radius: 6px;
  background: var(--sunken);
  font-family: var(--font-mono);
  font-size: 0.86em;
}
.notist-document pre {
  overflow-x: auto;
  padding: 16px 18px;
  border: 1px solid var(--border-soft);
  border-radius: var(--radius);
  background: var(--sunken);
  line-height: 1.7;
}
.notist-document pre code {
  padding: 0;
  border: 0;
  background: none;
  font-size: 13.5px;
}

/* block elements */
.notist-document blockquote {
  margin-left: 0;
  padding: 2px 0 2px 18px;
  border-left: 3px solid var(--border);
  color: var(--muted);
}
.notist-document blockquote footer { margin-top: 0.4em; font-size: 0.9em; }
.notist-rule {
  margin: 2.75em 0;
  border: 0;
  border-top: 1px solid var(--border);
}
.notist-pagebreak {
  margin: 2.75em 0;
  border: 0;
  border-top: 1px dashed var(--faint);
  break-after: page;
}
.notist-image { max-width: 100%; height: auto; border-radius: 8px; vertical-align: middle; }
.notist-figure figcaption {
  margin-top: 0.5em;
  color: var(--muted);
  font-size: 0.9em;
  text-align: center;
}
.notist-video { display: block; width: 100%; max-width: 100%; margin: 1.25em 0; border-radius: var(--radius); }
.notist-audio { display: block; width: 100%; margin: 1em 0; }
.notist-math { font-family: "Cambria Math", "STIX Two Math", serif; }
div.notist-math { margin: 1.1em 0; overflow-x: auto; text-align: center; }
.notist-citation { font-style: normal; white-space: nowrap; }

/* references */
.notist-reference-unresolved,
.notist-unresolved-call {
  color: var(--danger);
  text-decoration: underline wavy;
  text-decoration-thickness: 1px;
  text-underline-offset: 3px;
}

/* task lists */
.notist-task-list { padding-left: 0 !important; list-style: none; }
.notist-task-item {
  display: grid;
  grid-template-columns: 18px minmax(0, 1fr);
  gap: 9px;
  align-items: start;
}
.notist-task-item > input { margin: 0.45em 0 0; accent-color: var(--accent); }
.notist-task-item > p { margin-bottom: 0.5em; }

/* inline widgets */
.notist-keyboard {
  padding: 0.1em 0.42em;
  border: 1px solid var(--border);
  border-bottom-width: 2px;
  border-radius: 6px;
  background: var(--surface);
  box-shadow: var(--shadow-sm);
  font: 0.85em/1.4 var(--font-mono);
  white-space: nowrap;
}
.notist-sample { font-family: var(--font-mono); }
.notist-spoiler {
  padding: 0 0.2em;
  border-radius: 5px;
  background: var(--text);
  color: transparent;
  cursor: pointer;
  box-decoration-break: clone;
  -webkit-box-decoration-break: clone;
}
.notist-spoiler:hover, .notist-spoiler:focus {
  background: color-mix(in srgb, var(--text) 12%, transparent);
  color: inherit;
  outline: 1px solid var(--border);
}

/* outline block (in-document) */
.notist-outline {
  margin: 1.2em 0;
  padding: 14px 18px;
  border: 1px solid var(--border-soft);
  border-radius: var(--radius);
  background: var(--surface);
}
.notist-outline ol { margin: 0; padding-left: 1.4em; }
.notist-outline li + li { margin-top: 0.25em; }
.notist-outline-level-2 { margin-left: 0.9em; }
.notist-outline-level-3 { margin-left: 1.8em; }
.notist-outline-level-4 { margin-left: 2.7em; }
.notist-outline-level-5 { margin-left: 3.6em; }
.notist-outline-level-6 { margin-left: 4.5em; }

/* callouts */
.notist-callout {
  --callout-accent: var(--info);
  margin: 1.2em 0;
  padding: 13px 16px;
  border: 1px solid color-mix(in srgb, var(--callout-accent) 28%, transparent);
  border-left: 3px solid var(--callout-accent);
  border-radius: 8px;
  background: color-mix(in srgb, var(--callout-accent) 6%, transparent);
}
.notist-callout[data-notist-kind="tip"],
.notist-callout[data-notist-kind="success"] { --callout-accent: var(--success); }
.notist-callout[data-notist-kind="warning"],
.notist-callout[data-notist-kind="caution"] { --callout-accent: var(--warning); }
.notist-callout[data-notist-kind="danger"],
.notist-callout[data-notist-kind="error"],
.notist-callout[data-notist-kind="important"] { --callout-accent: var(--danger); }
.notist-callout-title {
  margin-bottom: 0.35em;
  color: var(--callout-accent);
  font-weight: 650;
}
.notist-callout > :last-child { margin-bottom: 0; }

/* details */
.notist-details {
  margin: 1.2em 0;
  padding: 12px 16px;
  border: 1px solid var(--border-soft);
  border-radius: var(--radius);
  background: var(--surface);
}
.notist-details summary {
  cursor: pointer;
  color: var(--text-strong);
  font-weight: 600;
}
.notist-details summary::marker { color: var(--faint); }
.notist-details[open] summary { margin-bottom: 0.5em; }
.notist-details > :last-child { margin-bottom: 0; }

/* footnotes */
.notist-footnote-ref { margin-inline: 0.12em; }
.notist-footnote-ref a { text-decoration: none; }
.notist-footnotes {
  margin-top: 2.5rem;
  padding-top: 1rem;
  border-top: 1px solid var(--border-soft);
  color: var(--muted);
  font-size: 0.9em;
}
.notist-footnotes ol { padding-left: 1.4em; }
.notist-footnote-backref { margin-inline-start: 0.45em; text-decoration: none; }

/* tables */
.notist-table-wrapper {
  max-width: 100%;
  margin: 0 0 1.2em;
  overflow-x: auto;
  border: 1px solid var(--border);
  border-radius: var(--radius);
}
.notist-table-wrapper table { width: 100%; border-collapse: collapse; font-size: 0.94em; }
.notist-figure > .notist-table-wrapper { margin: 0; }
.notist-table-wrapper th,
.notist-table-wrapper td {
  min-width: 5rem;
  padding: 9px 14px;
  border-bottom: 1px solid var(--border-soft);
  vertical-align: top;
  text-align: start;
}
.notist-table-wrapper tr:last-child th,
.notist-table-wrapper tr:last-child td { border-bottom: 0; }
.notist-table-wrapper thead th {
  background: var(--sunken);
  color: var(--text-strong);
  font-weight: 600;
}
.notist-table-wrapper tbody tr:hover td { background: color-mix(in srgb, var(--sunken) 45%, transparent); }
.notist-table-wrapper th > :first-child,
.notist-table-wrapper td > :first-child { margin-top: 0; }
.notist-table-wrapper th > :last-child,
.notist-table-wrapper td > :last-child { margin-bottom: 0; }
.notist-table-align-left { text-align: left; }
.notist-table-align-center { text-align: center; }
.notist-table-align-right { text-align: right; }

/* virtual module index */
.module-index {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(230px, 1fr));
  gap: 12px;
  margin: 1.4em 0;
  padding: 0;
  list-style: none;
}
.module-index a {
  display: block;
  height: 100%;
  padding: 15px 17px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--surface);
  color: var(--text);
  font-weight: 550;
  text-decoration: none;
  transition: border-color 0.15s ease, box-shadow 0.15s ease, translate 0.15s ease;
}
.module-index a:hover {
  border-color: var(--accent);
  box-shadow: var(--shadow-md);
  translate: 0 -1px;
}

/* ---------- print ---------- */
@media print {
  .site-sidebar, .page-rail, .topbar, .to-top, .skip-link, .live-status, .breadcrumb, .page-footer,
  .preview-chrome, .inspect-tooltip {
    display: none !important;
  }
  body { padding: 0; background: #fff; }
  body.enhanced .notist-document [data-notist-tag],
  body.enhanced .notist-document .notist-annotated,
  body.enhanced .notist-document .inspect-hover { outline: none !important; background: none; }
  .page-body { padding: 0; }
  .page-main { width: 100%; }
  .notist-document pre,
  .notist-document blockquote,
  .notist-callout,
  .notist-table-wrapper { break-inside: avoid; }
  .notist-pagebreak { visibility: hidden; margin: 0; break-after: page; }
}

@media (prefers-reduced-motion: reduce) {
  html { scroll-behavior: auto; }
  *, *::before, *::after { transition: none !important; animation: none !important; }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn render(root: &Path) -> RenderedWorkspaceRecord {
        let mut client =
            LocalNotistClient::connect(true, ClientKind::Test, root.to_path_buf()).unwrap();
        let opened = client
            .request(CoreRequest::OpenView {
                root: root.to_path_buf(),
                kind: ProtocolViewKind::Disk,
            })
            .unwrap();
        let CoreResponse::Opened { view_id, .. } = opened.response else {
            panic!("expected open view")
        };
        render_workspace(&mut client, view_id).unwrap()
    }

    #[test]
    fn builds_source_and_virtual_modules_with_relative_links() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
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
        let output = root.path().join("site");
        let rendered = render(root.path());
        let result = write_rendered_site(&rendered, &output, SiteOptions::default()).unwrap();

        assert_eq!(result.page_count, 4);
        assert!(rendered.evaluation_diagnostics.is_empty());
        let home = fs::read_to_string(output.join("index.html")).unwrap();
        let guide = fs::read_to_string(output.join("guide/index.html")).unwrap();
        let notes = fs::read_to_string(output.join("notes/index.html")).unwrap();
        assert!(home.contains("href=\"guide/\""));
        assert!(guide.contains("href=\"../\""));
        assert!(notes.contains("href=\"../notes/chapter%20one/\""));
        assert!(output.join("notes/chapter one/index.html").is_file());
        assert!(output.join("_notist/style.css").is_file());
        let site_script = fs::read_to_string(output.join("_notist/site.js")).unwrap();
        assert!(site_script.contains("notist-sidebar-scroll"));
        assert!(site_script.contains("sidebar.scrollTop"));
    }

    #[test]
    fn site_styles_are_copied_and_linked_after_the_builtin_sheet() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::create_dir_all(root.path().join("assets/theme")).unwrap();
        fs::write(
            root.path().join("Notist.toml"),
            "[site]\nstyles = [\"assets/theme/user.css\", \"assets/theme/user.css\"]",
        )
        .unwrap();
        fs::write(
            root.path().join("assets/theme/user.css"),
            ".user { color: darkblue; }",
        )
        .unwrap();
        fs::write(root.path().join("README.not"), "#heading[Home]").unwrap();
        fs::create_dir(root.path().join("guide")).unwrap();
        fs::write(root.path().join("guide/README.not"), "#heading[Guide]").unwrap();
        let output = root.path().join("site");

        let styles = notist_plugin_host::site_styles(Some(
            &fs::read_to_string(root.path().join("Notist.toml")).unwrap(),
        ))
        .unwrap();
        assert_eq!(styles, vec!["assets/theme/user.css"]);
        let rendered = render(root.path());
        let web_paths = copy_site_styles(root.path(), &output, &styles).unwrap();
        write_rendered_site_with_plugins(
            &rendered,
            &output,
            SiteOptions::default(),
            &[],
            &web_paths,
        )
        .unwrap();

        let copied =
            fs::read_to_string(output.join("_notist/styles/assets/theme/user.css")).unwrap();
        assert!(copied.contains(".user { color: darkblue; }"));
        let home = fs::read_to_string(output.join("index.html")).unwrap();
        let builtin = home.find("_notist/style.css").unwrap();
        let custom = home
            .find("href=\"_notist/styles/assets/theme/user.css\"")
            .unwrap();
        assert!(
            custom > builtin,
            "custom sheet must load after the built-in one"
        );
        // Deduplicated: the config listed the same sheet twice.
        assert_eq!(home.matches("_notist/styles/").count(), 1);
        let guide = fs::read_to_string(output.join("guide/index.html")).unwrap();
        assert!(guide.contains("href=\"../_notist/styles/assets/theme/user.css\""));
    }

    #[test]
    fn live_reload_script_releases_eventsource_when_page_is_hidden() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join("README.not"), "#heading[Home]").unwrap();
        let output = root.path().join("site");
        let rendered = render(root.path());
        write_rendered_site(&rendered, &output, SiteOptions { live_reload: true }).unwrap();

        let script = fs::read_to_string(output.join("_notist/reload.js")).unwrap();
        assert!(script.contains("addEventListener(\"pagehide\", closeEvents)"));
        assert!(script.contains("document.addEventListener(\"visibilitychange\""));
        assert!(script.contains("if (event.persisted) openEvents();"));
        assert!(script.contains("if (document.visibilityState !== \"hidden\") openEvents();"));

        let home = fs::read_to_string(output.join("index.html")).unwrap();
        assert!(home.contains("_notist/reload.js"));
    }

    #[test]
    fn preview_pages_include_the_enhanced_mode_chrome() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(
            root.path().join("README.not"),
            "#heading[Home]\n\n#let answer = 42\n#let double(x: Int) -> Int = x * 2",
        )
        .unwrap();
        let output = root.path().join("site");
        let rendered = render(root.path());

        write_rendered_site(
            &rendered,
            &output.join("preview"),
            SiteOptions { live_reload: true },
        )
        .unwrap();
        let inspect = fs::read_to_string(output.join("preview/_notist/inspect.js")).unwrap();
        assert!(inspect.contains("inspect-toggle"));
        assert!(inspect.contains("rail-panel-inspector"));
        assert!(inspect.contains("notist-enhanced"));
        let source = fs::read_to_string(output.join("preview/_notist/source.js")).unwrap();
        assert!(source.contains("source-toggle"));
        assert!(source.contains("notist-source"));
        assert!(source.contains("navigator.clipboard.writeText"));
        let home = fs::read_to_string(output.join("preview/index.html")).unwrap();
        assert!(home.contains("class=\"preview-chrome\""));
        assert!(home.contains("role=\"switch\""));
        assert!(home.contains("_notist/inspect.js"));
        assert!(home.contains("_notist/source.js"));
        // The inspector lives in a rail tab next to the TOC, even on pages
        // without headings (the rail is then hidden until enhanced mode).
        assert!(home.contains("class=\"page-rail\" data-empty-toc"));
        assert!(home.contains("id=\"rail-tab-inspector\""));
        assert!(home.contains("id=\"rail-panel-inspector\""));
        // Root bindings ship as embedded JSON for the inspector's symbol table.
        assert!(home.contains("id=\"notist-bindings\""));
        assert!(home.contains("\"name\":\"answer\""), "{home}");
        assert!(home.contains("Int = 42"), "{home}");
        assert!(home.contains("fn(x: Int) -> Int"), "{home}");
        // The source toggle owns the rendered/source switch, and the raw text
        // ships from the same snapshot that produced the fragment.
        assert!(home.contains("id=\"source-toggle\""));
        assert!(home.contains("id=\"source-panel\""));
        assert!(home.contains("id=\"notist-source\""));
        assert!(home.contains("#heading[Home]"), "{home}");
        assert!(home.contains("#let answer"), "{home}");
        let styles = fs::read_to_string(output.join("preview/_notist/style.css")).unwrap();
        assert!(styles.contains(".preview-chrome"));
        assert!(styles.contains(".rail-tab"));
        assert!(styles.contains(".inspector-panel"));
        assert!(styles.contains(".chrome-toggle"));
        assert!(styles.contains(".source-panel"));
        assert!(styles.contains("white-space: pre-wrap"));
        assert!(styles.contains("overflow-wrap: anywhere"));
        assert!(styles.contains("body.source-open .page-rail"));
        assert!(styles.contains("body.source-open.enhanced .page-rail[data-empty-toc]"));

        // Static builds stay clean: no preview chrome, no inspector script,
        // no bindings or source payloads, and no rail at all on a page
        // without TOC-level headings.
        write_rendered_site(&rendered, &output.join("static"), SiteOptions::default()).unwrap();
        let static_home = fs::read_to_string(output.join("static/index.html")).unwrap();
        assert!(!static_home.contains("preview-chrome"));
        assert!(!static_home.contains("rail-tab"));
        assert!(!static_home.contains("page-rail"));
        assert!(!static_home.contains("notist-bindings"));
        assert!(!static_home.contains("notist-source"));
        assert!(!static_home.contains("source-toggle"));
        assert!(!static_home.contains("source-panel"));
        assert!(!output.join("static/_notist/inspect.js").exists());
        assert!(!output.join("static/_notist/source.js").exists());
    }

    #[test]
    fn preview_virtual_modules_disable_the_source_toggle() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join("README.not"), "#heading[Home]").unwrap();
        fs::create_dir(root.path().join("notes")).unwrap();
        fs::write(root.path().join("notes/chapter.not"), "#heading[One]").unwrap();
        let output = root.path().join("site");
        let rendered = render(root.path());
        write_rendered_site(&rendered, &output, SiteOptions { live_reload: true }).unwrap();

        let home = fs::read_to_string(output.join("index.html")).unwrap();
        assert!(home.contains("id=\"source-toggle\""));
        assert!(home.contains("aria-controls=\"source-panel\""));
        assert!(home.contains("id=\"notist-source\""));
        assert!(home.contains("id=\"source-panel\""));
        assert!(!home.contains("This virtual module has no .not source file"));

        let notes = fs::read_to_string(output.join("notes/index.html")).unwrap();
        assert!(notes.contains("id=\"source-toggle\""));
        assert!(
            notes.contains("disabled title=\"This virtual module has no .not source file\""),
            "{notes}"
        );
        assert!(!notes.contains("id=\"notist-source\""));
        assert!(!notes.contains("id=\"source-panel\""));
    }

    #[test]
    fn preview_source_json_escapes_closing_script_tags() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(
            root.path().join("README.not"),
            "Text </script> and <b>raw</b> markup.",
        )
        .unwrap();
        let output = root.path().join("site");
        let rendered = render(root.path());
        write_rendered_site(&rendered, &output, SiteOptions { live_reload: true }).unwrap();

        let home = fs::read_to_string(output.join("index.html")).unwrap();
        assert!(home.contains("id=\"notist-source\""), "{home}");
        // The embedded JSON must never let source text close the script tag.
        assert!(home.contains("<\\/script>"), "{home}");
        // The rendered article escapes the same text independently.
        assert!(home.contains("&lt;/script&gt;"), "{home}");
    }

    #[test]
    fn page_shell_includes_breadcrumb_toc_and_semantic_labels() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::create_dir(root.path().join("guide")).unwrap();
        fs::write(root.path().join("README.not"), "#heading[Home]").unwrap();
        fs::write(
            root.path().join("guide/README.not"),
            "#heading[Guide]\n\n#heading(level=2)[Install]\n\nText.",
        )
        .unwrap();
        let output = root.path().join("site");
        let rendered = render(root.path());
        write_rendered_site(&rendered, &output, SiteOptions::default()).unwrap();

        let guide = fs::read_to_string(output.join("guide/index.html")).unwrap();
        assert!(guide.contains("aria-label=\"Breadcrumb\""));
        assert!(guide.contains("<title>Guide · "));
        assert!(guide.contains("class=\"page-toc\""));
        assert!(guide.contains("href=\"#Install\""));
        assert!(guide.contains("class=\"breadcrumb\""));
        let home = fs::read_to_string(output.join("index.html")).unwrap();
        assert!(!home.contains("aria-label=\"Breadcrumb\""));
        assert!(home.contains("aria-label=\"Site navigation\""));
    }

    #[test]
    fn builds_annotation_ids_as_label_targets() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
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
        let output = root.path().join("site");
        let rendered = render(root.path());
        write_rendered_site(&rendered, &output, SiteOptions::default()).unwrap();

        let home = fs::read_to_string(output.join("index.html")).unwrap();
        let guide = fs::read_to_string(output.join("guide/index.html")).unwrap();
        assert!(home.contains("href=\"guide/#intro\""));
        assert!(home.contains("id=\"home\""));
        assert!(guide.contains("id=\"intro\""));
    }

    #[test]
    fn builds_block_prefix_annotations_into_the_page() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(
            root.path().join("README.not"),
            "= Home\n\n@[bid,#wip,.hero,priority=1]\n== Section\n\n#[scoped]@sid,#tag-a,k=2 text.",
        )
        .unwrap();
        let output = root.path().join("site");
        let rendered = render(root.path());
        write_rendered_site(&rendered, &output, SiteOptions::default()).unwrap();

        let home = fs::read_to_string(output.join("index.html")).unwrap();
        // Block-prefix `@[...]`: id on the heading, attributes on the inline wrapper.
        assert!(home.contains("id=\"bid\""), "{home}");
        assert!(home.contains("notist-annotated hero"), "{home}");
        assert!(home.contains("data-notist-tag=\"wip\""), "{home}");
        assert!(home.contains("data-notist-priority=\"1\""), "{home}");
        // Postfix `@...` on a manual scope keeps working through the same table.
        assert!(home.contains("id=\"sid\""), "{home}");
        assert!(home.contains("data-notist-tag=\"tag-a\""), "{home}");
        assert!(home.contains("data-notist-k=\"2\""), "{home}");
    }

    #[test]
    fn copies_resource_files_and_links_them() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::create_dir(root.path().join("images")).unwrap();
        fs::write(
            root.path().join("README.not"),
            "= Home\n\nSee [[vault::images#logo.png]].",
        )
        .unwrap();
        fs::write(
            root.path().join("images/logo.png"),
            [0x89, 0x50, 0x4E, 0x47],
        )
        .unwrap();
        let output = root.path().join("site");
        let rendered = render(root.path());
        let result = write_rendered_site(&rendered, &output, SiteOptions::default()).unwrap();

        assert_eq!(rendered.resources.len(), 1);
        assert_eq!(result.page_count, 2);
        let copied = output.join("images/logo.png");
        assert!(copied.is_file());
        assert_eq!(fs::read(&copied).unwrap(), [0x89, 0x50, 0x4E, 0x47]);
        let home = fs::read_to_string(output.join("index.html")).unwrap();
        assert!(home.contains("href=\"images/logo.png\""), "{home}");
        let images = fs::read_to_string(output.join("images/index.html")).unwrap();
        assert!(images.contains("data-notist-kind=\"image\""), "{images}");
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
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        let canonical = dunce::canonicalize(root.path()).unwrap();

        let error = prepare_output_root(root.path(), &canonical).unwrap_err();

        assert!(error.to_string().contains("workspace root"));
    }

    #[test]
    fn explicit_clean_removes_only_the_selected_output_tree() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        let output = root.path().join("site");
        fs::create_dir_all(&output).unwrap();
        fs::write(output.join("stale.html"), "stale").unwrap();
        let output = dunce::canonicalize(output).unwrap();

        clean_output_root(&output).unwrap();

        assert!(output.is_dir());
        assert!(!output.join("stale.html").exists());
        assert!(root.path().is_dir());
    }

    #[test]
    fn render_workspace_orders_pages_from_module_attributes() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join("Notist.toml"), "").unwrap();
        fs::write(root.path().join("README.not"), "= Home").unwrap();
        fs::write(root.path().join("alpha.not"), "@![#top]\n= Alpha").unwrap();
        fs::write(root.path().join("beta.not"), "@![order = 10]\n= Beta").unwrap();
        fs::write(root.path().join("gamma.not"), "@![order = 5]\n= Gamma").unwrap();
        fs::write(root.path().join("delta.not"), "= Delta").unwrap();
        fs::write(root.path().join("zeta.not"), "= Zeta").unwrap();

        let rendered = render(root.path());

        // Root remains first in the rendered page list.
        assert_eq!(rendered.pages[0].module_segments, Vec::<String>::new());
        let top_level: Vec<Vec<&str>> = rendered
            .pages
            .iter()
            .filter(|page| page.module_segments.len() == 1)
            .map(|page| {
                page.module_segments
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(
            top_level,
            vec![
                vec!["alpha"],
                vec!["gamma"],
                vec!["beta"],
                vec!["delta"],
                vec!["zeta"],
            ]
        );
    }
}
