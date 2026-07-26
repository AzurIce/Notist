use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use percent_encoding::percent_decode_str;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(crate) struct ConvertResult {
    pub source: PathBuf,
    pub output: PathBuf,
    pub converted_files: usize,
    pub copied_assets: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct Document {
    source: PathBuf,
    relative: PathBuf,
    output_relative: PathBuf,
    module: String,
    headings: HeadingIndex,
}

#[derive(Debug, Clone, Default)]
struct HeadingIndex {
    ordered: Vec<String>,
    by_name: HashMap<String, String>,
}

struct Vault {
    documents: Vec<Document>,
    by_relative: HashMap<String, usize>,
    by_stem: HashMap<String, Vec<usize>>,
    assets_by_relative: HashMap<String, PathBuf>,
    assets_by_name: HashMap<String, Vec<PathBuf>>,
}

pub(crate) fn run(
    source: &Path,
    output: &Path,
    force: bool,
) -> Result<ConvertResult, Box<dyn std::error::Error>> {
    let source = dunce::canonicalize(source)?;
    if !source.is_dir() {
        return Err(format!("source vault `{}` is not a directory", source.display()).into());
    }

    let output = absolute_path(output)?;
    if output.starts_with(&source) {
        return Err("output directory must not be inside the source vault".into());
    }
    if output.exists() && !force && fs::read_dir(&output)?.next().is_some() {
        return Err(format!(
            "output directory `{}` is not empty; pass --force to merge into it",
            output.display()
        )
        .into());
    }
    fs::create_dir_all(&output)?;

    let mut files = Vec::new();
    collect_files(&source, &mut files)?;
    files.sort();
    let vault = Vault::index(source.clone(), &files)?;
    let mut warnings = Vec::new();

    for (index, document) in vault.documents.iter().enumerate() {
        let markdown = fs::read_to_string(&document.source).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "failed to read `{}` as UTF-8: {error}",
                    document.source.display()
                ),
            )
        })?;
        let converted = Renderer {
            vault: &vault,
            document_index: index,
            warnings: &mut warnings,
            heading_cursor: 0,
            list_stack: Vec::new(),
        }
        .render(&markdown);
        let destination = output.join(&document.output_relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(destination, converted)?;
    }

    let mut copied_assets = 0;
    for file in &files {
        if is_markdown(file) {
            continue;
        }
        let relative = file.strip_prefix(&source)?;
        let destination = output.join(sanitize_relative_path(relative));
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(file, destination)?;
        copied_assets += 1;
    }
    let manifest = output.join("Notist.toml");
    if !manifest.exists() {
        fs::write(manifest, "")?;
    }

    warnings.sort();
    warnings.dedup();
    Ok(ConvertResult {
        source,
        output,
        converted_files: vault.documents.len(),
        copied_assets,
        warnings,
    })
}

impl Vault {
    fn index(root: PathBuf, files: &[PathBuf]) -> Result<Self, Box<dyn std::error::Error>> {
        let mut documents = Vec::new();
        let mut modules = BTreeMap::new();
        let mut output_paths = BTreeMap::new();
        for source in files.iter().filter(|path| is_markdown(path)) {
            let relative = source.strip_prefix(&root)?.to_path_buf();
            let output_relative = sanitize_relative_path(&relative.with_extension("not"));
            if let Some(previous) =
                output_paths.insert(path_key(&output_relative), relative.clone())
            {
                return Err(format!(
                    "`{}` and `{}` both map to output path `{}`",
                    previous.display(),
                    relative.display(),
                    output_relative.display()
                )
                .into());
            }
            let module = module_for(&output_relative);
            if let Some(previous) = modules.insert(module.clone(), relative.clone()) {
                return Err(format!(
                    "`{}` and `{}` both map to Notist module `{module}`",
                    previous.display(),
                    relative.display()
                )
                .into());
            }
            let markdown = fs::read_to_string(source)?;
            documents.push(Document {
                source: source.clone(),
                relative,
                output_relative,
                module,
                headings: index_headings(&markdown),
            });
        }

        let mut by_relative = HashMap::new();
        let mut by_stem: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, document) in documents.iter().enumerate() {
            by_relative.insert(path_key(&document.relative), index);
            by_relative.insert(path_key(&document.relative.with_extension("")), index);
            if document
                .relative
                .file_stem()
                .is_some_and(|stem| stem.eq_ignore_ascii_case("README"))
                && let Some(parent) = document.relative.parent()
            {
                by_relative.insert(path_key(parent), index);
            }
            if let Some(stem) = document.relative.file_stem() {
                by_stem
                    .entry(stem.to_string_lossy().to_lowercase())
                    .or_default()
                    .push(index);
            }
        }
        let mut assets_by_relative = HashMap::new();
        let mut assets_by_name: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for source in files.iter().filter(|path| !is_markdown(path)) {
            let relative = source.strip_prefix(&root)?.to_path_buf();
            let output_relative = sanitize_relative_path(&relative);
            if let Some(previous) =
                output_paths.insert(path_key(&output_relative), relative.clone())
            {
                return Err(format!(
                    "`{}` and `{}` both map to output path `{}`",
                    previous.display(),
                    relative.display(),
                    output_relative.display()
                )
                .into());
            }
            assets_by_relative.insert(path_key(&relative), output_relative.clone());
            if let Some(name) = relative.file_name() {
                assets_by_name
                    .entry(name.to_string_lossy().to_lowercase())
                    .or_default()
                    .push(output_relative);
            }
        }
        Ok(Self {
            documents,
            by_relative,
            by_stem,
            assets_by_relative,
            assets_by_name,
        })
    }

    fn resolve_document(&self, current: usize, target: &str) -> Result<Option<usize>, String> {
        let decoded = percent_decode_str(target).decode_utf8_lossy();
        let target = decoded.replace('\\', "/");
        let path = Path::new(target.trim());
        let mut candidates = Vec::new();
        if let Some(parent) = self.documents[current].relative.parent() {
            candidates.push(normalize_relative(&parent.join(path)));
        }
        candidates.push(normalize_relative(path));
        for candidate in candidates {
            let key = path_key(&candidate);
            if let Some(index) = self.by_relative.get(&key) {
                return Ok(Some(*index));
            }
            if candidate.extension().is_none() {
                let with_md = candidate.with_extension("md");
                if let Some(index) = self.by_relative.get(&path_key(&with_md)) {
                    return Ok(Some(*index));
                }
                let readme = candidate.join("README.md");
                if let Some(index) = self.by_relative.get(&path_key(&readme)) {
                    return Ok(Some(*index));
                }
            }
        }
        let stem = path
            .file_stem()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .to_lowercase();
        match self.by_stem.get(&stem).map(Vec::as_slice) {
            Some([only]) => Ok(Some(*only)),
            Some(many) if many.len() > 1 => Err(format!(
                "ambiguous Wiki target `{target}` matches {} documents",
                many.len()
            )),
            _ => Ok(None),
        }
    }

    fn reference(&self, current: usize, target: &str, heading: Option<&str>) -> Option<String> {
        let index = self.resolve_document(current, target).ok().flatten()?;
        let mut reference = self.documents[index].module.clone();
        if let Some(heading) = heading.filter(|heading| !heading.is_empty()) {
            let label = self.heading_label(index, heading)?;
            reference.push('#');
            reference.push_str(&label);
        }
        Some(reference)
    }

    fn heading_label(&self, document: usize, heading: &str) -> Option<String> {
        let lookup = heading.trim().trim_start_matches('^').to_lowercase();
        self.documents[document]
            .headings
            .by_name
            .get(&lookup)
            .or_else(|| {
                self.documents[document]
                    .headings
                    .by_name
                    .get(&markdown_slug(heading))
            })
            .cloned()
    }

    fn asset_destination(&self, current: usize, target: &str) -> Option<String> {
        if target.contains("://") || target.starts_with("data:") || target.starts_with("mailto:") {
            return None;
        }
        let (path, suffix) = split_link_suffix(target);
        let decoded = percent_decode_str(path).decode_utf8_lossy();
        let source_path = Path::new(decoded.as_ref());
        let mut candidates = Vec::new();
        if let Some(parent) = self.documents[current].relative.parent() {
            candidates.push(normalize_relative(&parent.join(source_path)));
        }
        candidates.push(normalize_relative(source_path));
        let destination = candidates
            .iter()
            .find_map(|candidate| self.assets_by_relative.get(&path_key(candidate)))
            .cloned()
            .or_else(|| {
                let name = source_path.file_name()?.to_string_lossy().to_lowercase();
                match self.assets_by_name.get(&name).map(Vec::as_slice) {
                    Some([only]) => Some(only.clone()),
                    _ => None,
                }
            })?;
        let from = self.documents[current]
            .output_relative
            .parent()
            .unwrap_or_else(|| Path::new(""));
        let mut relative = relative_path(from, &destination)
            .to_string_lossy()
            .replace('\\', "/");
        relative.push_str(suffix);
        Some(relative)
    }
}

struct Renderer<'a> {
    vault: &'a Vault,
    document_index: usize,
    warnings: &'a mut Vec<String>,
    heading_cursor: usize,
    list_stack: Vec<bool>,
}

impl Renderer<'_> {
    fn render(mut self, markdown: &str) -> String {
        let mut compact = Vec::new();
        for event in Parser::new_ext(markdown, markdown_options()) {
            if let Event::Text(text) = event {
                if let Some(Event::Text(previous)) = compact.last_mut() {
                    let mut joined = previous.to_string();
                    joined.push_str(&text);
                    *previous = CowStr::from(joined);
                } else {
                    compact.push(Event::Text(text));
                }
            } else {
                compact.push(event);
            }
        }
        let mut events = compact.into_iter();
        let mut output = String::new();
        while let Some(event) = events.next() {
            self.event(event, &mut events, &mut output);
        }
        output
    }

    fn event<'a, I>(&mut self, event: Event<'a>, events: &mut I, output: &mut String)
    where
        I: Iterator<Item = Event<'a>>,
    {
        match event {
            Event::Start(tag) => self.container(tag, events, output),
            Event::Text(text) => self.text(&text, output),
            Event::Code(code) => {
                output.push('`');
                output.push_str(&code.replace('`', "``"));
                output.push('`');
            }
            Event::InlineMath(math) => write_call(output, "math", &[("text", &math)]),
            Event::DisplayMath(math) => {
                output.push_str("#math(text=");
                string_literal(&math, output);
                output.push_str(", block=true)");
                output.push_str("\n\n");
            }
            Event::SoftBreak => output.push('\n'),
            Event::HardBreak => output.push_str("\\\n"),
            Event::Rule => output.push_str("#rule()\n\n"),
            Event::Html(html) | Event::InlineHtml(html) => {
                write_call(output, "raw", &[("text", &html), ("lang", "html")]);
            }
            Event::FootnoteReference(name) => {
                output.push_str("\\[");
                escape_markup(&name, output);
                output.push_str("\\]");
            }
            Event::TaskListMarker(checked) => {
                output.push_str(if checked { "[x] " } else { "[ ] " })
            }
            Event::End(_) => {}
        }
    }

    fn container<'a, I>(&mut self, tag: Tag<'a>, events: &mut I, output: &mut String)
    where
        I: Iterator<Item = Event<'a>>,
    {
        match tag {
            Tag::Paragraph => {
                self.until(TagEnd::Paragraph, events, output);
                output.push_str("\n\n");
            }
            Tag::Heading { level, .. } => {
                output.push_str("#heading(level=");
                output.push_str(heading_number(level));
                output.push_str(")[");
                self.until(TagEnd::Heading(level), events, output);
                output.push(']');
                if let Some(label) = self.vault.documents[self.document_index]
                    .headings
                    .ordered
                    .get(self.heading_cursor)
                {
                    output.push('@');
                    output.push_str(label);
                }
                self.heading_cursor += 1;
                output.push_str("\n\n");
            }
            Tag::BlockQuote(kind) => {
                output.push_str("#quote[\n");
                self.until(TagEnd::BlockQuote(kind), events, output);
                output.push_str("]\n\n");
            }
            Tag::CodeBlock(kind) => {
                let language = match &kind {
                    CodeBlockKind::Fenced(language) => language.as_ref(),
                    CodeBlockKind::Indented => "",
                };
                let mut body = String::new();
                for event in events.by_ref() {
                    match event {
                        Event::End(TagEnd::CodeBlock) => break,
                        Event::Text(text) | Event::Code(text) => body.push_str(&text),
                        _ => {}
                    }
                }
                let fence = "`".repeat(longest_backtick_run(&body).saturating_add(1).max(3));
                output.push_str(&fence);
                output.push_str(language);
                output.push('\n');
                output.push_str(&body);
                if !output.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str(&fence);
                output.push_str("\n\n");
            }
            Tag::List(start) => {
                self.list_stack.push(start.is_some());
                output.push_str(if start.is_some() { "#enum[" } else { "#list[" });
                self.until(TagEnd::List(start.is_some()), events, output);
                self.list_stack.pop();
                output.push_str("]\n\n");
            }
            Tag::Item => {
                output.push_str(if self.list_stack.last() == Some(&true) {
                    "#enum::item["
                } else {
                    "#list::item["
                });
                self.until(TagEnd::Item, events, output);
                trim_parbreak(output);
                output.push(']');
            }
            Tag::Emphasis => {
                output.push('_');
                self.until(TagEnd::Emphasis, events, output);
                output.push('_');
            }
            Tag::Strong => {
                output.push('*');
                self.until(TagEnd::Strong, events, output);
                output.push('*');
            }
            Tag::Strikethrough => {
                output.push_str("~~");
                self.until(TagEnd::Strikethrough, events, output);
                output.push_str("~~");
            }
            Tag::Link {
                dest_url, title, ..
            } => {
                let mut body = String::new();
                self.until(TagEnd::Link, events, &mut body);
                self.link(&dest_url, &title, &body, output);
            }
            Tag::Image {
                dest_url, title, ..
            } => {
                let mut alt = String::new();
                self.until(TagEnd::Image, events, &mut alt);
                let source = self
                    .vault
                    .asset_destination(self.document_index, &dest_url)
                    .unwrap_or_else(|| dest_url.to_string());
                output.push_str("#image(source=");
                string_literal(&source, output);
                output.push_str(", alt=");
                string_literal(&plain_text(&alt), output);
                if !title.is_empty() {
                    output.push_str(", title=");
                    string_literal(&title, output);
                }
                output.push(')');
            }
            Tag::Table(alignments) => {
                output.push_str("#table(columns=");
                output.push_str(&alignments.len().to_string());
                output.push_str(", header=true)[");
                self.until(TagEnd::Table, events, output);
                output.push_str("]\n\n");
            }
            Tag::TableHead => self.until(TagEnd::TableHead, events, output),
            Tag::TableRow => self.until(TagEnd::TableRow, events, output),
            Tag::TableCell => {
                output.push_str("#table::cell[");
                self.until(TagEnd::TableCell, events, output);
                output.push(']');
            }
            Tag::HtmlBlock => {
                let mut html = String::new();
                self.until(TagEnd::HtmlBlock, events, &mut html);
                output.push_str("#code(text=");
                string_literal(&html, output);
                output.push_str(", lang=\"html\", block=true)");
                output.push_str("\n\n");
            }
            other => {
                let end = other.to_end();
                self.until(end, events, output);
            }
        }
    }

    fn until<'a, I>(&mut self, end: TagEnd, events: &mut I, output: &mut String)
    where
        I: Iterator<Item = Event<'a>>,
    {
        while let Some(event) = events.next() {
            if matches!(&event, Event::End(found) if *found == end) {
                break;
            }
            self.event(event, events, output);
        }
    }

    fn text(&mut self, text: &str, output: &mut String) {
        let mut rest = text;
        while let Some(start) = rest.find("[[") {
            escape_markup(&rest[..start], output);
            let embed = start > 0 && rest.as_bytes()[start - 1] == b'!';
            if embed && output.ends_with("\\!") {
                output.truncate(output.len() - 2);
            }
            let after = &rest[start + 2..];
            let Some(end) = after.find("]]") else {
                escape_markup(&rest[start..], output);
                return;
            };
            self.wiki(&after[..end], embed, output);
            rest = &after[end + 2..];
        }
        escape_markup(rest, output);
    }

    fn wiki(&mut self, body: &str, embed: bool, output: &mut String) {
        let (target, alias) = body.split_once('|').unwrap_or((body, ""));
        let (path, heading) = target.split_once('#').unwrap_or((target, ""));
        let path = path.trim();
        if embed && is_asset_target(path) {
            let source = self
                .vault
                .asset_destination(self.document_index, path)
                .unwrap_or_else(|| path.to_owned());
            output.push_str("#image(source=");
            string_literal(&source, output);
            output.push_str(", alt=");
            string_literal(alias, output);
            if let Ok(width) = alias.trim().parse::<u32>() {
                output.push_str(", width=");
                output.push_str(&width.to_string());
            }
            output.push(')');
            return;
        }
        if let Some(destination) = self.vault.asset_destination(self.document_index, path) {
            output.push_str("#link(destination=");
            string_literal(&destination, output);
            output.push_str(")[");
            escape_markup(if alias.is_empty() { path } else { alias }, output);
            output.push(']');
            return;
        }
        let reference = if path.is_empty() {
            self.vault
                .heading_label(self.document_index, heading)
                .map(|label| {
                    format!(
                        "{}#{label}",
                        self.vault.documents[self.document_index].module
                    )
                })
        } else {
            self.vault
                .reference(self.document_index, path, Some(heading))
        };
        if let Some(reference) = reference {
            output.push_str("[[");
            output.push_str(&reference);
            output.push_str("]]");
        } else {
            self.warn(format!("unresolved Wiki target `{target}`"));
            escape_markup(if alias.is_empty() { target } else { alias }, output);
        }
    }

    fn link(&mut self, destination: &str, title: &str, body: &str, output: &mut String) {
        if destination.is_empty() {
            output.push_str(body);
            return;
        }
        let (path, fragment) = destination.split_once('#').unwrap_or((destination, ""));
        let external = is_external_destination(destination);
        let resolvable = !external
            && !path.is_empty()
            && self
                .vault
                .resolve_document(self.document_index, path)
                .ok()
                .flatten()
                .is_some();
        let doc_like = !external
            && (path.is_empty()
                || resolvable
                || Path::new(path)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("md")));
        if doc_like {
            let reference = if path.is_empty() {
                self.vault
                    .heading_label(self.document_index, fragment)
                    .map(|label| {
                        format!(
                            "{}#{label}",
                            self.vault.documents[self.document_index].module
                        )
                    })
            } else {
                self.vault
                    .reference(self.document_index, path, Some(fragment))
            };
            if let Some(reference) = reference {
                output.push_str("[[");
                output.push_str(&reference);
                output.push_str("]]");
                return;
            }
            self.warn(format!("unresolved document link `{destination}`"));
        }
        let destination = self
            .vault
            .asset_destination(self.document_index, destination)
            .unwrap_or_else(|| destination.to_owned());
        output.push_str("#link(destination=");
        string_literal(&destination, output);
        if !title.is_empty() {
            output.push_str(", title=");
            string_literal(title, output);
        }
        output.push_str(")[");
        output.push_str(body);
        output.push(']');
    }

    fn warn(&mut self, warning: String) {
        let path = &self.vault.documents[self.document_index].relative;
        self.warnings.push(format!("{}: {warning}", path.display()));
    }
}

fn markdown_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_MATH
}

fn index_headings(markdown: &str) -> HeadingIndex {
    let mut index = HeadingIndex::default();
    let mut events = Parser::new_ext(markdown, markdown_options());
    let mut counts = HashMap::<String, usize>::new();
    while let Some(event) = events.next() {
        if !matches!(event, Event::Start(Tag::Heading { .. })) {
            continue;
        }
        let mut text = String::new();
        for event in events.by_ref() {
            match event {
                Event::End(TagEnd::Heading(_)) => break,
                Event::Text(value) | Event::Code(value) => text.push_str(&value),
                _ => {}
            }
        }
        let base = identifier(&text);
        let count = counts.entry(base.clone()).or_insert(0);
        *count += 1;
        let label = if *count == 1 {
            base
        } else {
            format!("{base}-{}", *count)
        };
        index
            .by_name
            .entry(text.trim().to_lowercase())
            .or_insert_with(|| label.clone());
        index
            .by_name
            .entry(markdown_slug(&text))
            .or_insert_with(|| label.clone());
        index.ordered.push(label);
    }
    index
}

fn module_for(relative: &Path) -> String {
    let mut parts: Vec<String> = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if let Some(file) = parts.pop() {
        let stem = Path::new(&file)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if !stem.eq_ignore_ascii_case("README") {
            parts.push(stem);
        }
    }
    if parts.is_empty() {
        "vault".into()
    } else {
        format!("vault::{}", parts.join("::"))
    }
}

fn identifier(text: &str) -> String {
    let mut output = String::new();
    let mut separator = false;
    for character in text.trim().chars() {
        if character.is_alphanumeric() || matches!(character, '_' | '-') {
            if separator && !output.is_empty() && !output.ends_with('-') {
                output.push('-');
            }
            output.push(character);
            separator = false;
        } else {
            separator = true;
        }
    }
    while output.ends_with('-') {
        output.pop();
    }
    if output.is_empty() {
        "section".into()
    } else {
        output
    }
}

fn markdown_slug(text: &str) -> String {
    let mut output = String::new();
    for character in text.trim().chars() {
        if character.is_alphanumeric() || matches!(character, '_' | '-') {
            output.extend(character.to_lowercase());
        } else if character.is_whitespace() && !output.is_empty() {
            output.push('-');
        }
    }
    while output.ends_with('-') {
        output.pop();
    }
    output
}

fn is_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
    })
}

fn is_asset_target(target: &str) -> bool {
    Path::new(target).extension().is_some_and(|extension| {
        matches!(
            extension.to_string_lossy().to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "avif"
        )
    })
}

fn is_external_destination(destination: &str) -> bool {
    destination.contains("://")
        || destination.starts_with("mailto:")
        || destination.starts_with("tel:")
        || destination.starts_with("data:")
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn absolute_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

fn normalize_relative(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            Component::Normal(part) => output.push(part),
            _ => {}
        }
    }
    output
}

fn sanitize_relative_path(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        if let Component::Normal(part) = component {
            let mut sanitized = String::new();
            for character in part.to_string_lossy().chars() {
                if character.is_control() || matches!(character, '[' | ']' | '#') {
                    if !sanitized.ends_with('-') {
                        sanitized.push('-');
                    }
                } else {
                    sanitized.push(character);
                }
            }
            if matches!(sanitized.as_str(), "vault" | "self" | "super") {
                sanitized.insert(0, '_');
            }
            output.push(sanitized);
        }
    }
    output
}

fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let shared = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let mut output = PathBuf::new();
    for _ in shared..from.len() {
        output.push("..");
    }
    for component in &to[shared..] {
        output.push(component.as_os_str());
    }
    output
}

fn split_link_suffix(destination: &str) -> (&str, &str) {
    destination
        .find(['#', '?'])
        .map_or((destination, ""), |index| destination.split_at(index))
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn escape_markup(text: &str, output: &mut String) {
    for character in text.chars() {
        if character.is_ascii_punctuation() {
            output.push('\\');
        }
        output.push(character);
    }
}

fn string_literal(text: &str, output: &mut String) {
    output.push('"');
    for character in text.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character => output.push(character),
        }
    }
    output.push('"');
}

fn write_call(output: &mut String, name: &str, arguments: &[(&str, &str)]) {
    output.push('#');
    output.push_str(name);
    output.push('(');
    for (index, (key, value)) in arguments.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        output.push_str(key);
        output.push('=');
        string_literal(value, output);
    }
    output.push(')');
}

fn longest_backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for character in text.chars() {
        if character == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

fn plain_text(markup: &str) -> String {
    markup.replace('\\', "")
}

fn trim_parbreak(output: &mut String) {
    while output.ends_with("\n\n") {
        output.truncate(output.len() - 1);
    }
}

fn heading_number(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "1",
        HeadingLevel::H2 => "2",
        HeadingLevel::H3 => "3",
        HeadingLevel::H4 => "4",
        HeadingLevel::H5 => "5",
        HeadingLevel::H6 => "6",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_vault_links_and_assets() {
        let source = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let output = output.path().join("converted");
        fs::create_dir_all(source.path().join("images")).unwrap();
        fs::create_dir_all(source.path().join("guide")).unwrap();
        fs::write(
            source.path().join("README.md"),
            "# Home\n\nSee [[Guide#Install|setup]].\n\n![Flow](images/flow.png)\n",
        )
        .unwrap();
        fs::write(
            source.path().join("guide/Guide.md"),
            "# Install\n\n[Home](../README.md#Home)\n",
        )
        .unwrap();
        fs::write(source.path().join("images/flow.png"), b"png").unwrap();

        let result = run(source.path(), &output, false).unwrap();
        assert_eq!(result.converted_files, 2);
        assert_eq!(result.copied_assets, 1);
        let home = fs::read_to_string(output.join("README.not")).unwrap();
        assert!(home.contains("#heading(level=1)[Home]@Home"), "{home}");
        assert!(home.contains("[[vault::guide::Guide#Install]]"), "{home}");
        assert!(
            home.contains("#image(source=\"images/flow.png\", alt=\"Flow\")"),
            "{home}"
        );
        let guide = fs::read_to_string(output.join("guide/Guide.not")).unwrap();
        assert!(guide.contains("[[vault#Home]]"), "{guide}");
        assert!(output.join("images/flow.png").is_file());
        assert!(output.join("Notist.toml").is_file());
        let workspace = notist_analysis::WorkspaceSnapshot::load(&output).unwrap();
        assert!(
            workspace.diagnostics().is_empty(),
            "{:?}",
            workspace.diagnostics()
        );
    }

    #[test]
    fn converts_lists_and_pads_short_markdown_table_rows() {
        let source = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap().path().join("converted");
        fs::write(
            source.path().join("README.md"),
            "- parent\n  - child\n\n1. first\n2. second\n\n| a | b | c |\n| --- | --- | --- |\n| one | two |\n",
        )
        .unwrap();

        run(source.path(), &output, false).unwrap();
        let converted = fs::read_to_string(output.join("README.not")).unwrap();
        assert!(
            converted.contains("#list[#list::item[parent"),
            "{converted}"
        );
        assert!(
            converted.contains("#enum[#enum::item[first]"),
            "{converted}"
        );
        assert!(converted.contains("#table::cell[]"), "{converted}");
        let workspace = notist_analysis::WorkspaceSnapshot::load(&output).unwrap();
        assert!(
            workspace.diagnostics().is_empty(),
            "{:?}",
            workspace.diagnostics()
        );
    }

    #[test]
    fn rejects_output_inside_source() {
        let source = tempfile::tempdir().unwrap();
        let error = run(source.path(), &source.path().join("out"), false).unwrap_err();
        assert!(error.to_string().contains("must not be inside"));
    }

    #[test]
    fn sanitizes_module_paths_and_rewrites_local_asset_paths() {
        let source = tempfile::tempdir().unwrap();
        let output_root = tempfile::tempdir().unwrap();
        let output = output_root.path().join("converted");
        fs::create_dir_all(source.path().join("[Course]")).unwrap();
        fs::write(
            source.path().join("README.md"),
            "[[[Course]/README]] and [external](https://example.test/guide.md)",
        )
        .unwrap();
        fs::write(
            source.path().join("[Course]/README.md"),
            "# Course\n\n![diagram](image.png)",
        )
        .unwrap();
        fs::write(source.path().join("[Course]/image.png"), b"png").unwrap();

        let result = run(source.path(), &output, false).unwrap();
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        let directory = output.join("-Course-");
        assert!(directory.join("README.not").is_file());
        assert!(directory.join("image.png").is_file());
        let root = fs::read_to_string(output.join("README.not")).unwrap();
        assert!(root.contains("[[vault::-Course-]]"), "{root}");
        assert!(root.contains("https://example.test/guide.md"), "{root}");
        let course = fs::read_to_string(directory.join("README.not")).unwrap();
        assert!(course.contains("source=\"image.png\""), "{course}");
        let workspace = notist_analysis::WorkspaceSnapshot::load(&output).unwrap();
        assert!(
            workspace.diagnostics().is_empty(),
            "{:?}",
            workspace.diagnostics()
        );
    }
}
