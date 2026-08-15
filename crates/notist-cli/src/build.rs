use std::error::Error;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::ColorChoice;
use notist_model::ModulePath;
use notist_service::protocol::ClientKind;
use notist_service::{
    CoreRequest, CoreResponse, ProtocolViewKind, RenderedHeadingRecord, RenderedWorkspaceRecord,
    ServiceViewId,
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
    let result = write_rendered_site(&rendered, &output, SiteOptions::default())?;
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
    let CoreResponse::RenderedWorkspace(rendered) = reply.response else {
        return Err("service returned an unexpected render response".into());
    };
    Ok(rendered)
}

pub(crate) fn write_rendered_site(
    rendered: &RenderedWorkspaceRecord,
    output: &Path,
    options: SiteOptions,
) -> Result<RenderedBuildResult, Box<dyn Error>> {
    fs::create_dir_all(output.join("_notist"))?;
    fs::write(output.join("_notist/style.css"), STYLES)?;
    fs::write(output.join("_notist/site.js"), SITE_SCRIPT)?;
    if options.live_reload {
        fs::write(output.join("_notist/reload.js"), LIVE_RELOAD_SCRIPT)?;
    }
    let pages = rendered
        .pages
        .iter()
        .map(|page| PageView {
            module: ModulePath::from_segments(page.module_segments.clone()),
            title: page.title.as_deref(),
            headings: &page.headings,
            fragment: &page.fragment,
        })
        .collect::<Vec<_>>();
    for page in &pages {
        let html = page_shell(&rendered.site_name, page, &pages, options);
        let page_path = module_output_dir(output, &page.module).join("index.html");
        if let Some(parent) = page_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(page_path, html)?;
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
    html.push_str("_notist/style.css\">\n<script src=\"");
    html.push_str(&asset_prefix);
    html.push_str("_notist/site.js\" defer></script>\n");
    if options.live_reload {
        html.push_str("<script src=\"");
        html.push_str(&asset_prefix);
        html.push_str("_notist/reload.js\" defer></script>\n");
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
    html.push_str("</article>\n<footer class=\"page-footer\"><span>Built with Notist</span><span class=\"page-module\">");
    escape_html(&mut html, &page.module.to_string());
    html.push_str("</span></footer>\n</main>\n");
    html.push_str(&page_toc(page));
    html.push_str("\n</div>\n</div>\n<button class=\"icon-button to-top\" id=\"to-top\" type=\"button\" aria-label=\"Back to top\">");
    html.push_str(ICON_ARROW_UP);
    html.push_str("</button>\n</body>\n</html>\n");
    html
}

fn module_tree(current: &ModulePath, pages: &[PageView<'_>]) -> String {
    let mut output = String::from("<nav class=\"module-tree\" aria-label=\"Modules\"><ol>");
    if let Some(home) = pages
        .iter()
        .find(|page| page.module.segments().is_empty())
    {
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

fn tree_item(output: &mut String, current: &ModulePath, pages: &[PageView<'_>], page: &PageView<'_>) {
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
    let mut output = String::from("<nav class=\"breadcrumb\" aria-label=\"Breadcrumb\"><ol><li><a href=\"");
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

fn page_toc(page: &PageView<'_>) -> String {
    if !page
        .headings
        .iter()
        .any(|heading| (2..=5).contains(&heading.level))
    {
        return String::new();
    }
    let mut output = String::from(
        "<aside class=\"page-toc\" aria-label=\"On this page\"><div class=\"toc-title\">On this page</div><ol>",
    );
    for heading in page.headings {
        if !(2..=5).contains(&heading.level) {
            continue;
        }
        write!(output, "<li style=\"--toc-level:{}\"><a href=\"#", heading.level).unwrap();
        escape_attribute(&mut output, &heading.id);
        output.push_str("\">");
        escape_html(&mut output, &heading.text);
        output.push_str("</a></li>");
    }
    output.push_str("</ol></aside>");
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
  document.body.append(pill);

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
.page-toc {
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
@media (max-width: 1240px) {
  .page-toc { display: none; }
  .page-body { gap: 0; }
}

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

.live-status {
  position: fixed;
  left: 20px;
  bottom: 20px;
  z-index: 70;
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
  .site-sidebar, .page-toc, .topbar, .to-top, .skip-link, .live-status, .breadcrumb, .page-footer {
    display: none !important;
  }
  body { padding: 0; background: #fff; }
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
        assert!(
            script.contains("if (document.visibilityState !== \"hidden\") openEvents();")
        );

        let home = fs::read_to_string(output.join("index.html")).unwrap();
        assert!(home.contains("_notist/reload.js"));
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
    fn copies_resource_files_and_links_them() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::create_dir(root.path().join("images")).unwrap();
        fs::write(
            root.path().join("README.not"),
            "= Home\n\nSee [[vault::images#logo.png]].",
        )
        .unwrap();
        fs::write(root.path().join("images/logo.png"), [0x89, 0x50, 0x4E, 0x47]).unwrap();
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
}
