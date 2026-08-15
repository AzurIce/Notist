use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use notist_analysis::{
    DiagnosticKind, DiagnosticSeverity as AnalysisSeverity, MissingReason, RefTarget,
    WorkspaceSnapshot,
};
use notist_model::{ModulePath, TextRange};
use regex::RegexBuilder;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{
    BooleanQuery, BoostQuery, ConstScoreQuery, Occur, Query, RegexQuery, TermQuery,
};
use tantivy::schema::{Field, IndexRecordOption, STORED, STRING, Schema, TEXT, Value};
use tantivy::{Index, IndexReader, ReloadPolicy, TantivyDocument, Term, doc};
use unicode_normalization::UnicodeNormalization;

use crate::{SnapshotIdentity, ViewKind};

pub const DEFAULT_MAX_BYTES: usize = 16 * 1024;
pub const HARD_MAX_BYTES: usize = 64 * 1024;
pub const MIN_MAX_BYTES: usize = 4 * 1024;
pub const CURSOR_MAX_BYTES: usize = 4096;
pub const SEARCH_DEFAULT_LIMIT: usize = 8;
pub const DEFAULT_LIMIT: usize = 20;
pub const HARD_LIMIT: usize = 100;
pub const DEFAULT_SNIPPET_BYTES: usize = 256;
pub const MAX_SNIPPET_BYTES: usize = 2048;
pub const READ_DEFAULT_LINES: usize = 120;
const MAX_SEARCH_CANDIDATES: usize = 10_000;
const REGEX_SCAN_DEADLINE: Duration = Duration::from_secs(2);
pub const RANKING_VERSION: &str = "bm25-v3";
pub const TOKENIZER_VERSION: &str = "notist-unicode-v1";
pub const INDEX_SCHEMA_VERSION: u32 = 4;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PageRequest {
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub cursor: Option<String>,
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            limit: None,
            max_bytes: None,
            cursor: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PageInfo {
    pub requested_limit: usize,
    pub applied_limit: usize,
    pub returned: usize,
    pub has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BudgetInfo {
    pub requested_bytes: usize,
    pub applied_bytes: usize,
    pub logical_bytes: usize,
    pub exhausted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoverageInfo {
    pub complete: bool,
    pub stop_reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryPage<T> {
    pub snapshot: SnapshotIdentity,
    pub items: Vec<T>,
    pub page: PageInfo,
    pub budget: BudgetInfo,
    pub coverage: CoverageInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search: Option<SearchPageMetadata>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hints: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchPageMetadata {
    pub group_by: SearchGroup,
    pub ordering: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ranking_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_stamp: Option<IndexStamp>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub expansion_limited: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<String>,
}

impl ToolError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            hint: None,
            candidates: Vec::new(),
        }
    }

    pub fn retryable(mut self, hint: impl Into<String>) -> Self {
        self.retryable = true;
        self.hint = Some(hint.into());
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Selector {
    Module {
        module: String,
        #[serde(default)]
        label: Option<String>,
    },
    Path {
        path: PathBuf,
        #[serde(default)]
        label: Option<String>,
    },
}

impl Selector {
    pub fn parse(value: &str) -> Self {
        let (head, label) = value
            .split_once('#')
            .map_or((value, None), |(head, label)| {
                (head, Some(label.to_owned()))
            });
        if head == "vault" || head.starts_with("vault::") {
            Self::Module {
                module: head.to_owned(),
                label,
            }
        } else {
            Self::Path {
                path: PathBuf::from(head),
                label,
            }
        }
    }

    fn fingerprint(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Location {
    pub module: String,
    pub relative_path: PathBuf,
    pub byte_range: super::request::ByteRange,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_range: Option<LineRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub source_fingerprint: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DefinitionQuery {
    pub path: PathBuf,
    pub offset: usize,
    #[serde(default)]
    pub expected_fingerprint: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LineRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModuleItem {
    pub module: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<PathBuf>,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_fingerprint: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModulesQuery {
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub kind: ModuleKind,
    #[serde(default)]
    pub page: PageRequest,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleKind {
    #[default]
    Any,
    Source,
    Virtual,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutlineQuery {
    pub selector: Selector,
    #[serde(default = "default_outline_depth")]
    pub depth: u8,
    #[serde(default)]
    pub page: PageRequest,
}

fn default_outline_depth() -> u8 {
    6
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutlineItem {
    pub name: String,
    pub level: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub location: Location,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_range: Option<super::request::ByteRange>,
    pub subtree_range: super::request::ByteRange,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ReadWindow {
    #[serde(default)]
    pub from_line: Option<usize>,
    #[serde(default)]
    pub lines: Option<usize>,
    #[serde(default)]
    pub byte_range: Option<super::request::ByteRange>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadQuery {
    pub selector: Selector,
    #[serde(default)]
    pub window: ReadWindow,
    #[serde(default)]
    pub page: PageRequest,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceChunk {
    pub location: Location,
    pub source: String,
    pub reached_end: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceDirection {
    #[default]
    Incoming,
    Outgoing,
    Both,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReferencesQuery {
    pub selector: Selector,
    #[serde(default)]
    pub direction: ReferenceDirection,
    #[serde(default)]
    pub include_definition: bool,
    #[serde(default = "default_snippet_bytes")]
    pub snippet_bytes: usize,
    #[serde(default)]
    pub page: PageRequest,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReferenceItem {
    pub source: String,
    pub target: String,
    pub direction: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_kind: Option<String>,
    pub location: Location,
    pub excerpt: String,
    pub excerpt_truncated: bool,
    pub is_definition: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Lexical,
    Exact,
    Fuzzy,
    Regex,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchOperator {
    #[default]
    All,
    Any,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchGroup {
    Source,
    Section,
    Match,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchField {
    Title,
    Heading,
    Id,
    Module,
    Path,
    Tag,
    Body,
    Raw,
    Comment,
}

impl SearchField {
    pub fn defaults() -> Vec<Self> {
        vec![
            Self::Title,
            Self::Heading,
            Self::Id,
            Self::Module,
            Self::Path,
            Self::Tag,
            Self::Body,
            Self::Raw,
        ]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    #[serde(default)]
    pub mode: SearchMode,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default = "SearchField::defaults")]
    pub fields: Vec<SearchField>,
    #[serde(default)]
    pub operator: SearchOperator,
    #[serde(default)]
    pub group_by: Option<SearchGroup>,
    #[serde(default)]
    pub ignore_case: bool,
    #[serde(default = "default_fuzzy_distance")]
    pub fuzzy_distance: u8,
    #[serde(default = "default_wait_index_ms")]
    pub wait_index_ms: u64,
    #[serde(default = "default_snippet_bytes")]
    pub snippet_bytes: usize,
    #[serde(default)]
    pub page: PageRequest,
}

impl SearchQuery {
    pub fn applied_group_by(&self) -> SearchGroup {
        self.group_by.unwrap_or(match self.mode {
            SearchMode::Lexical | SearchMode::Fuzzy => SearchGroup::Source,
            SearchMode::Exact | SearchMode::Regex => SearchGroup::Match,
        })
    }
}

fn default_fuzzy_distance() -> u8 {
    1
}

fn default_wait_index_ms() -> u64 {
    2000
}

fn default_snippet_bytes() -> usize {
    DEFAULT_SNIPPET_BYTES
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchHit {
    pub location: Location,
    pub matched_field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_range: Option<super::request::ByteRange>,
    pub unit_range: super::request::ByteRange,
    pub excerpt: String,
    pub excerpt_range: super::request::ByteRange,
    pub excerpt_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<u64>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IndexStamp {
    pub source_fingerprint: String,
    pub schema_version: u32,
    pub tokenizer_version: String,
    pub ranking_version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexStatusRecord {
    pub health: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stamp: Option<IndexStamp>,
    pub unit_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatusRecord {
    pub root: PathBuf,
    pub source_count: usize,
    pub module_count: usize,
    pub diagnostic_count: usize,
    pub runtime_mode: String,
    pub view_kind: String,
    pub snapshot: SnapshotIdentity,
    pub index: IndexStatusRecord,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DiagnosticsQuery {
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub summary_only: bool,
    #[serde(default)]
    pub severity: DiagnosticSeverity,
    #[serde(default)]
    pub page: PageRequest,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    #[default]
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticSummary {
    pub checked_sources: usize,
    pub total_diagnostics: usize,
    pub error_count: usize,
    pub counts_by_code: HashMap<String, usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticItem {
    pub code: String,
    pub severity: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excerpt_range: Option<super::request::ByteRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excerpt_line_start: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticsResult {
    pub summary: DiagnosticSummary,
    pub diagnostics: QueryPage<DiagnosticItem>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebugSection {
    #[default]
    Modules,
    References,
    Semantic,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DebugQuery {
    #[serde(default)]
    pub section: DebugSection,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub page: PageRequest,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DebugItem {
    pub module: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<super::request::ByteRange>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CursorPayload {
    version: u32,
    operation: String,
    vault_fingerprint: String,
    view_kind: String,
    #[serde(default)]
    daemon_instance: String,
    #[serde(default)]
    view_id: u64,
    source_fingerprint: String,
    query_fingerprint: String,
    offset: usize,
}

#[derive(Clone, Debug)]
struct ResolvedSource<'a> {
    module: &'a notist_analysis::Module,
    source: &'a notist_analysis::SourceInput,
    label: Option<String>,
    selection: TextRange,
}

pub fn list_modules(
    workspace: &WorkspaceSnapshot,
    snapshot: &SnapshotIdentity,
    query: &ModulesQuery,
) -> Result<QueryPage<ModuleItem>, ToolError> {
    let prefix = query.prefix.as_deref();
    let mut items = workspace
        .modules()
        .filter(|module| {
            prefix.is_none_or(|prefix| {
                let path = module.logical_path.to_string();
                path == prefix || path.starts_with(&format!("{prefix}::"))
            })
        })
        .filter(|module| match query.kind {
            ModuleKind::Any => true,
            ModuleKind::Source => module.file_id.is_some(),
            ModuleKind::Virtual => module.file_id.is_none(),
        })
        .map(|module| {
            let title = module
                .file_id
                .and_then(|file_id| workspace.document_symbols(file_id).into_iter().next())
                .map(|symbol| symbol.name);
            ModuleItem {
                module: module.logical_path.to_string(),
                relative_path: module
                    .source_path
                    .as_deref()
                    .map(|path| relative_path(workspace.root(), path)),
                kind: if module.file_id.is_some() {
                    "source"
                } else {
                    "virtual"
                }
                .into(),
                title,
                source_fingerprint: module
                    .file_id
                    .and_then(|file_id| workspace.source(file_id))
                    .map(|source| fingerprint(&source.text)),
            }
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.module.cmp(&right.module));
    page(
        snapshot,
        "modules",
        &serde_json::to_string(&(query.prefix.clone(), query.kind)).unwrap(),
        &query.page,
        DEFAULT_LIMIT,
        items,
    )
}

pub fn outline(
    workspace: &WorkspaceSnapshot,
    snapshot: &SnapshotIdentity,
    query: &OutlineQuery,
) -> Result<QueryPage<OutlineItem>, ToolError> {
    if !(1..=6).contains(&query.depth) {
        return Err(ToolError::new(
            "invalid_argument",
            "outline depth must be between 1 and 6",
        ));
    }
    let resolved = resolve_source(workspace, &query.selector)?;
    let symbols = workspace
        .document_symbols(resolved.source.file_id)
        .into_iter()
        .filter(|symbol| symbol.level <= query.depth)
        .collect::<Vec<_>>();
    let mut items = Vec::new();
    for (index, symbol) in symbols.iter().enumerate() {
        let parent_range = symbols[..index]
            .iter()
            .rev()
            .find(|candidate| candidate.level < symbol.level)
            .map(|candidate| candidate.range.into());
        let subtree_end = symbols[index + 1..]
            .iter()
            .find(|candidate| candidate.level <= symbol.level)
            .map_or(resolved.selection.end, |candidate| candidate.range.start);
        items.push(OutlineItem {
            id: workspace
                .labels()
                .iter()
                .find(|label| {
                    label.file_id == resolved.source.file_id
                        && label.scope_range.start <= symbol.range.start
                        && symbol.range.end <= label.scope_range.end
                })
                .map(|label| label.name.clone())
                .or_else(|| Some(symbol.name.clone())),
            location: location(
                workspace,
                resolved.module,
                resolved.source,
                symbol.range,
                None,
            ),
            name: symbol.name.clone(),
            level: symbol.level,
            parent_range,
            subtree_range: TextRange::new(symbol.range.start, subtree_end).into(),
        });
    }
    items.sort_by_key(|item| item.location.byte_range.start);
    page(
        snapshot,
        "outline",
        &format!("{}:{}", query.selector.fingerprint(), query.depth),
        &query.page,
        HARD_LIMIT,
        items,
    )
}

pub fn read_source(
    workspace: &WorkspaceSnapshot,
    snapshot: &SnapshotIdentity,
    query: &ReadQuery,
) -> Result<QueryPage<SourceChunk>, ToolError> {
    let resolved = resolve_source(workspace, &query.selector)?;
    if query.window.byte_range.is_some()
        && (query.window.from_line.is_some() || query.window.lines.is_some())
    {
        return Err(ToolError::new(
            "invalid_argument",
            "line and byte windows are mutually exclusive",
        ));
    }
    if query.window.lines.is_some() && query.window.from_line.is_none() {
        return Err(ToolError::new(
            "invalid_argument",
            "lines requires from-line",
        ));
    }
    if query
        .window
        .lines
        .is_some_and(|lines| lines == 0 || lines > 1000)
    {
        return Err(ToolError::new(
            "invalid_argument",
            "lines must be between 1 and 1000",
        ));
    }
    let source = &resolved.source.text;
    let mut selection = resolved.selection;
    if let Some(range) = query.window.byte_range {
        if range.start > range.end || range.end > source.len() {
            return Err(ToolError::new(
                "invalid_argument",
                "byte range is outside the selected source",
            ));
        }
        selection = TextRange::new(range.start, range.end);
    } else if let Some(from_line) = query.window.from_line {
        let starts = line_starts(source);
        let start_index = from_line.saturating_sub(1);
        let Some(&start) = starts.get(start_index) else {
            return Err(ToolError::new(
                "invalid_argument",
                "from-line is outside the selected source",
            ));
        };
        let count = query.window.lines.unwrap_or(READ_DEFAULT_LINES);
        let end = starts
            .get(start_index.saturating_add(count))
            .copied()
            .unwrap_or(source.len());
        selection = TextRange::new(start.max(selection.start), end.min(selection.end));
    }
    let query_fp = format!("{}:{:?}", query.selector.fingerprint(), query.window);
    let start = cursor_offset(snapshot, "read", &query_fp, &query.page)?.unwrap_or(selection.start);
    if start < selection.start || start > selection.end {
        return Err(ToolError::new(
            "invalid_cursor",
            "read cursor is outside the selected range",
        )
        .with_hint(
            "resend the original selector and read window unchanged with cursor, or omit cursor to restart",
        ));
    }
    let max_bytes = applied_max_bytes(&query.page)?;
    let source_budget = max_bytes.saturating_sub(2048).max(256);
    let page_end = if query.window.from_line.is_none() && query.window.byte_range.is_none() {
        let starts = line_starts(source);
        let current_line = starts
            .partition_point(|line_start| *line_start <= start)
            .saturating_sub(1);
        starts
            .get(current_line.saturating_add(READ_DEFAULT_LINES))
            .copied()
            .unwrap_or(selection.end)
            .min(selection.end)
    } else {
        selection.end
    };
    let end = floor_char_boundary(source, (start + source_budget).min(page_end));
    let end = if end == start && start < selection.end {
        source[start..]
            .char_indices()
            .nth(1)
            .map_or(selection.end, |(offset, _)| start + offset)
    } else {
        end
    };
    let reached_end = end >= selection.end;
    let chunk = SourceChunk {
        location: location(
            workspace,
            resolved.module,
            resolved.source,
            TextRange::new(start, end),
            resolved.label.clone(),
        ),
        source: source[start..end].to_owned(),
        reached_end,
    };
    let next_cursor = (!reached_end).then(|| encode_cursor("read", snapshot, &query_fp, end));
    let items = vec![chunk];
    let logical_bytes = serde_json::to_vec(&items)
        .map(|value| value.len())
        .unwrap_or(0);
    Ok(QueryPage {
        snapshot: snapshot.clone(),
        items,
        page: PageInfo {
            requested_limit: 1,
            applied_limit: 1,
            returned: 1,
            has_more: !reached_end,
            next_cursor,
        },
        budget: BudgetInfo {
            requested_bytes: query.page.max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
            applied_bytes: max_bytes,
            logical_bytes,
            exhausted: !reached_end,
        },
        coverage: CoverageInfo {
            complete: reached_end,
            stop_reason: if reached_end {
                "complete"
            } else {
                "byte_budget"
            }
            .into(),
        },
        search: None,
        hints: continuation_hints(!reached_end),
    })
}

/// Converts a resolved RefTarget into its serialized record shape (D0004).
pub fn ref_target_record(
    workspace: &WorkspaceSnapshot,
    target: RefTarget,
) -> super::request::RefTargetRecord {
    use notist_analysis::ResourceKind;
    let module_name =
        |module_id| workspace.module_by_id(module_id).map(|module| module.logical_path.to_string());
    match target {
        RefTarget::Module(module_id) => super::request::RefTargetRecord {
            kind: "module".into(),
            module: module_name(module_id),
            ..Default::default()
        },
        RefTarget::Scope { module, id } => super::request::RefTargetRecord {
            kind: "scope".into(),
            module: module_name(module),
            id: Some(id),
            ..Default::default()
        },
        RefTarget::Resource {
            module,
            name,
            kind,
        } => super::request::RefTargetRecord {
            kind: "resource".into(),
            module: module_name(module),
            name: Some(name),
            resource_kind: Some(
                match kind {
                    ResourceKind::Image => "image",
                    ResourceKind::File => "file",
                }
                .into(),
            ),
            ..Default::default()
        },
        RefTarget::External(url) => super::request::RefTargetRecord {
            kind: "external".into(),
            url: Some(url),
            ..Default::default()
        },
        RefTarget::Missing(reason) => super::request::RefTargetRecord {
            kind: "missing".into(),
            reason: Some(
                match reason {
                    MissingReason::Nonexistent => "nonexistent",
                    MissingReason::Ambiguous => "ambiguous",
                    MissingReason::Unsupported => "unsupported",
                }
                .into(),
            ),
            ..Default::default()
        },
    }
}

pub fn references(
    workspace: &WorkspaceSnapshot,
    snapshot: &SnapshotIdentity,
    query: &ReferencesQuery,
) -> Result<QueryPage<ReferenceItem>, ToolError> {
    validate_snippet(query.snippet_bytes)?;
    let resolved = resolve_source(workspace, &query.selector)?;
    let target_label = resolved.label.as_deref();
    let mut items = Vec::new();
    if matches!(
        query.direction,
        ReferenceDirection::Incoming | ReferenceDirection::Both
    ) {
        for reference in workspace.references().iter().filter(|reference| {
            reference.target_module_id == resolved.module.id
                && reference.target_label.as_deref() == target_label
        }) {
            if let Some(source) = workspace.source(reference.source_file_id)
                && let Some(module) = workspace.module_at(reference.source_file_id)
            {
                let (excerpt, _, truncated) =
                    excerpt(&source.text, reference.range, query.snippet_bytes);
                items.push(ReferenceItem {
                    source: module.logical_path.to_string(),
                    target: resolved.module.logical_path.to_string(),
                    direction: "incoming".into(),
                    relation: None,
                    url: None,
                    target_kind: None,
                    location: location(workspace, module, source, reference.range, None),
                    excerpt,
                    excerpt_truncated: truncated,
                    is_definition: false,
                });
            }
        }
    }
    if matches!(
        query.direction,
        ReferenceDirection::Outgoing | ReferenceDirection::Both
    ) {
        for reference in workspace.references().iter().filter(|reference| {
            reference.source_module_id == resolved.module.id
                && target_label.is_none_or(|label| {
                    resolved.selection.start <= reference.range.start
                        && reference.range.end <= resolved.selection.end
                        && !label.is_empty()
                })
        }) {
            let (excerpt, _, truncated) =
                excerpt(&resolved.source.text, reference.range, query.snippet_bytes);
            let target_kind = reference.target_label.as_deref().map_or("module", |label| {
                match workspace.resolve_module_label(&reference.target_module, label) {
                    RefTarget::Resource { .. } => "resource",
                    _ => "scope",
                }
            });
            items.push(ReferenceItem {
                source: resolved.module.logical_path.to_string(),
                target: reference.target_module.to_string(),
                direction: "outgoing".into(),
                relation: Some("reference".into()),
                url: Some(reference.url.clone()),
                target_kind: Some(target_kind.into()),
                location: location(
                    workspace,
                    resolved.module,
                    resolved.source,
                    reference.range,
                    resolved.label.clone(),
                ),
                excerpt,
                excerpt_truncated: truncated,
                is_definition: false,
            });
        }
    }
    if query.include_definition {
        items.push(ReferenceItem {
            source: resolved.module.logical_path.to_string(),
            target: resolved.module.logical_path.to_string(),
            direction: "definition".into(),
            relation: None,
            url: None,
            target_kind: None,
            location: location(
                workspace,
                resolved.module,
                resolved.source,
                resolved.selection,
                resolved.label.clone(),
            ),
            excerpt: String::new(),
            excerpt_truncated: false,
            is_definition: true,
        });
    }
    items.sort_by(|left, right| {
        left.location.module.cmp(&right.location.module).then(
            left.location
                .byte_range
                .start
                .cmp(&right.location.byte_range.start),
        )
    });
    page(
        snapshot,
        "references",
        &format!(
            "{}:{:?}:{}",
            query.selector.fingerprint(),
            query.direction,
            query.include_definition
        ),
        &query.page,
        DEFAULT_LIMIT,
        items,
    )
}

pub fn definition(
    workspace: &WorkspaceSnapshot,
    query: &DefinitionQuery,
) -> Result<Option<Location>, ToolError> {
    let absolute = if query.path.is_absolute() {
        query.path.clone()
    } else {
        workspace.root().join(&query.path)
    };
    let absolute = dunce::canonicalize(&absolute).map_err(|_| {
        ToolError::new(
            "not_found",
            format!("source `{}` was not found", query.path.display()),
        )
    })?;
    if !absolute.starts_with(workspace.root()) {
        return Err(ToolError::new(
            "invalid_selector",
            "source path escapes the Vault",
        ));
    }
    let file_id = workspace
        .file_id(&absolute)
        .ok_or_else(|| ToolError::new("not_found", "source is not part of the captured Vault"))?;
    let source = workspace.source(file_id).unwrap();
    if query
        .expected_fingerprint
        .as_deref()
        .is_some_and(|expected| expected != fingerprint(&source.text))
    {
        return Err(
            ToolError::new("snapshot_changed", "source fingerprint does not match")
                .retryable("read the current source and retry with its fingerprint"),
        );
    }
    if query.offset > source.text.len() || !source.text.is_char_boundary(query.offset) {
        return Err(ToolError::new(
            "invalid_argument",
            "offset is not a UTF-8 boundary in the selected source",
        ));
    }
    let Some(target) = workspace.definition_at(file_id, query.offset) else {
        return Ok(None);
    };
    let Some(target_file) = target.file_id else {
        return Ok(None);
    };
    let Some(target_source) = workspace.source(target_file) else {
        return Ok(None);
    };
    let Some(target_module) = workspace.module_at(target_file) else {
        return Ok(None);
    };
    Ok(Some(location(
        workspace,
        target_module,
        target_source,
        target.range.unwrap_or(TextRange::new(0, 0)),
        target.annotation.map(|annotation| annotation.name),
    )))
}

impl From<AnalysisSeverity> for DiagnosticSeverity {
    fn from(severity: AnalysisSeverity) -> Self {
        match severity {
            AnalysisSeverity::Error => Self::Error,
            AnalysisSeverity::Warning => Self::Warning,
            AnalysisSeverity::Info => Self::Info,
        }
    }
}

fn severity_rank(severity: DiagnosticSeverity) -> u8 {
    match severity {
        DiagnosticSeverity::Error => 2,
        DiagnosticSeverity::Warning => 1,
        DiagnosticSeverity::Info => 0,
    }
}

pub fn diagnostics(
    workspace: &WorkspaceSnapshot,
    snapshot: &SnapshotIdentity,
    query: &DiagnosticsQuery,
) -> Result<DiagnosticsResult, ToolError> {
    let mut counts = HashMap::new();
    let mut error_count = 0usize;
    let mut items = Vec::new();
    for diagnostic in workspace.diagnostics() {
        if let Some(scope) = query.scope.as_deref()
            && diagnostic
                .source_path
                .as_deref()
                .and_then(|path| workspace.module_for_source(path))
                .is_some_and(|module| !module.logical_path.to_string().starts_with(scope))
        {
            continue;
        }
        let code = diagnostic_code(&diagnostic.kind).to_owned();
        *counts.entry(code.clone()).or_insert(0) += 1;
        if diagnostic.kind.severity() == AnalysisSeverity::Error {
            error_count += 1;
        }
        if query.summary_only {
            continue;
        }
        if severity_rank(diagnostic.kind.severity().into()) < severity_rank(query.severity) {
            continue;
        }
        let location = diagnostic.source_path.as_deref().and_then(|path| {
            let module = workspace.module_for_source(path)?;
            let source = workspace.source(module.file_id?)?;
            Some(location(
                workspace,
                module,
                source,
                diagnostic.range.unwrap_or(TextRange::new(0, 0)),
                None,
            ))
        });
        let excerpt = location.as_ref().and_then(|location| {
            let path = workspace.root().join(&location.relative_path);
            let source = workspace
                .module_for_source(&path)
                .and_then(|module| workspace.source(module.file_id?))?;
            let (text, range, _) = excerpt(&source.text, location.byte_range.into(), 512);
            Some((
                text,
                super::request::ByteRange::from(range),
                line_range(&source.text, range).start,
            ))
        });
        items.push(DiagnosticItem {
            code,
            severity: diagnostic.kind.severity_label().into(),
            message: diagnostic.message.clone(),
            hint: diagnostic.kind.hint().map(str::to_owned),
            location,
            excerpt: excerpt.as_ref().map(|(text, _, _)| text.clone()),
            excerpt_range: excerpt.as_ref().map(|(_, range, _)| *range),
            excerpt_line_start: excerpt.map(|(_, _, line)| line),
        });
    }
    let summary = DiagnosticSummary {
        error_count,
        checked_sources: workspace
            .sources()
            .filter(|source| {
                query.scope.as_deref().is_none_or(|scope| {
                    workspace
                        .module_at(source.file_id)
                        .is_some_and(|module| module.logical_path.to_string().starts_with(scope))
                })
            })
            .count(),
        total_diagnostics: counts.values().sum(),
        counts_by_code: counts,
    };
    let diagnostics = page(
        snapshot,
        "diagnostics",
        &format!("{:?}:{:?}", query.scope, query.severity),
        &query.page,
        DEFAULT_LIMIT,
        items,
    )?;
    Ok(DiagnosticsResult {
        summary,
        diagnostics,
    })
}

pub fn debug_inspect(
    workspace: &WorkspaceSnapshot,
    snapshot: &SnapshotIdentity,
    query: &DebugQuery,
) -> Result<QueryPage<DebugItem>, ToolError> {
    let accepts = |module: &str| {
        query
            .module
            .as_deref()
            .is_none_or(|filter| module == filter)
    };
    let mut items = Vec::new();
    match query.section {
        DebugSection::Modules => {
            for module in workspace.modules() {
                let name = module.logical_path.to_string();
                if accepts(&name) {
                    items.push(DebugItem {
                        module: name,
                        kind: if module.file_id.is_some() {
                            "source"
                        } else {
                            "virtual"
                        }
                        .into(),
                        name: None,
                        target: None,
                        range: None,
                    });
                }
            }
        }
        DebugSection::References => {
            for reference in workspace.references() {
                let module = reference.source_module.to_string();
                if accepts(&module) {
                    items.push(DebugItem {
                        module,
                        kind: "reference".into(),
                        name: reference.target_label.clone(),
                        target: Some(reference.target_module.to_string()),
                        range: Some(reference.range.into()),
                    });
                }
            }
        }
        DebugSection::Semantic => {
            for module in workspace
                .modules()
                .filter(|module| module.file_id.is_some())
            {
                let module_name = module.logical_path.to_string();
                if !accepts(&module_name) {
                    continue;
                }
                for symbol in workspace.document_symbols(module.file_id.unwrap()) {
                    items.push(DebugItem {
                        module: module_name.clone(),
                        kind: "heading".into(),
                        name: Some(symbol.name),
                        target: None,
                        range: Some(symbol.range.into()),
                    });
                }
                for label in workspace
                    .labels()
                    .iter()
                    .filter(|label| label.module == module.logical_path)
                {
                    items.push(DebugItem {
                        module: module_name.clone(),
                        kind: "annotation".into(),
                        name: Some(label.name.clone()),
                        target: None,
                        range: Some(label.range.into()),
                    });
                }
            }
        }
    }
    items.sort_by(|left, right| {
        left.module.cmp(&right.module).then(
            left.range
                .as_ref()
                .map(|range| range.start)
                .cmp(&right.range.as_ref().map(|range| range.start)),
        )
    });
    page(
        snapshot,
        "debug_inspect",
        &format!("{:?}:{:?}", query.section, query.module),
        &query.page,
        DEFAULT_LIMIT,
        items,
    )
}

pub fn exact_or_regex_search(
    workspace: &WorkspaceSnapshot,
    snapshot: &SnapshotIdentity,
    query: &SearchQuery,
) -> Result<QueryPage<SearchHit>, ToolError> {
    validate_search(query)?;
    let group_by = query.applied_group_by();
    let requested_limit = query.page.limit.unwrap_or(SEARCH_DEFAULT_LIMIT);
    if requested_limit == 0 || requested_limit > HARD_LIMIT {
        return Err(ToolError::new(
            "budget_too_large",
            format!("limit must be between 1 and {HARD_LIMIT}"),
        ));
    }
    applied_max_bytes(&query.page)?;
    let offset =
        cursor_offset(snapshot, "search", &search_fingerprint(query), &query.page)?.unwrap_or(0);
    if offset >= MAX_SEARCH_CANDIDATES {
        return Err(ToolError::new(
            "query_limit",
            format!("search is limited to the first {MAX_SEARCH_CANDIDATES} candidates"),
        )
        .retryable("narrow --scope or --fields, or use a more selective query"));
    }
    let candidate_target = offset
        .saturating_add(requested_limit)
        .saturating_add(1)
        .min(MAX_SEARCH_CANDIDATES + 1);
    let deadline = (query.mode == SearchMode::Regex).then(|| Instant::now() + REGEX_SCAN_DEADLINE);
    let regex = match query.mode {
        SearchMode::Exact => RegexBuilder::new(&regex::escape(&query.query))
            .case_insensitive(query.ignore_case)
            .build(),
        SearchMode::Regex => RegexBuilder::new(&query.query)
            .case_insensitive(query.ignore_case)
            .size_limit(2 * 1024 * 1024)
            .build(),
        _ => {
            return Err(ToolError::new(
                "invalid_argument",
                "ranked search requires an index",
            ));
        }
    }
    .map_err(|error| ToolError::new("invalid_regex", error.to_string()))?;
    let mut hits = Vec::new();
    let mut grouped = BTreeSet::new();
    let mut modules = workspace
        .modules()
        .filter(|module| module.file_id.is_some())
        .collect::<Vec<_>>();
    modules.sort_by(|left, right| left.logical_path.cmp(&right.logical_path));
    let mut scan_complete = true;
    'modules: for module in modules {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(ToolError::new(
                "query_timeout",
                "regex scan exceeded the 2 second deadline",
            )
            .retryable("narrow --scope or --fields, or use --exact/lexical search"));
        }
        if !in_scope(&module.logical_path.to_string(), &query.scopes) {
            continue;
        }
        let source = workspace.source(module.file_id.unwrap()).unwrap();
        let mut seen = BTreeSet::new();
        for (field, value, source_range, direct_source) in
            searchable_regions(workspace, module, source, &query.fields)
        {
            for matched in regex.find_iter(&value) {
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    return Err(ToolError::new(
                        "query_timeout",
                        "regex scan exceeded the 2 second deadline",
                    )
                    .retryable("narrow --scope or --fields, or use --exact/lexical search"));
                }
                let range = if direct_source {
                    TextRange::new(
                        source_range.start + matched.start(),
                        source_range.start + matched.end(),
                    )
                } else {
                    source_range
                };
                if !seen.insert((range.start, range.end)) {
                    continue;
                }
                if !grouped.insert(search_group_key(workspace, module, range, group_by)) {
                    continue;
                }
                let (text, excerpt_range, truncated) =
                    excerpt(&source.text, range, query.snippet_bytes);
                hits.push(SearchHit {
                    location: location(workspace, module, source, range, None),
                    matched_field: field.clone(),
                    match_range: direct_source.then(|| super::request::ByteRange::from(range)),
                    unit_range: source_range.into(),
                    excerpt: text,
                    excerpt_range: excerpt_range.into(),
                    excerpt_truncated: truncated,
                    score: None,
                });
                if hits.len() >= candidate_target {
                    scan_complete = false;
                    break 'modules;
                }
            }
        }
    }
    let mut result = page(
        snapshot,
        "search",
        &search_fingerprint(query),
        &query.page,
        SEARCH_DEFAULT_LIMIT,
        hits,
    )?;
    result.search = Some(SearchPageMetadata {
        group_by,
        ordering: "source".into(),
        ranking_version: None,
        index_stamp: None,
        expansion_limited: false,
    });
    if !scan_complete && !result.page.has_more {
        result.coverage.complete = false;
        result.coverage.stop_reason = "query_limit".into();
    }
    add_empty_search_hint(&mut result, query);
    Ok(result)
}

fn searchable_regions(
    workspace: &WorkspaceSnapshot,
    module: &notist_analysis::Module,
    source: &notist_analysis::SourceInput,
    fields: &[SearchField],
) -> Vec<(String, String, TextRange, bool)> {
    let fields = if fields.is_empty() {
        SearchField::defaults()
    } else {
        fields.to_vec()
    };
    let mut regions = Vec::new();
    let raw_ranges = module
        .parse
        .as_ref()
        .map(|parse| {
            parse
                .raw_literals()
                .into_iter()
                .map(|raw| raw.payload_range)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let comment_ranges = comment_ranges(&source.text);
    let symbols = workspace.document_symbols(source.file_id);
    let mut excluded = raw_ranges.clone();
    excluded.extend(comment_ranges.iter().copied());
    excluded.extend(symbols.iter().map(|symbol| symbol.range));
    let excluded = merge_ranges(excluded);
    if fields.contains(&SearchField::Body) {
        for range in complement_ranges(source.text.len(), &excluded) {
            regions.push((
                "body".into(),
                source.text[range.start..range.end].into(),
                range,
                true,
            ));
        }
    }
    if fields.contains(&SearchField::Raw) {
        for range in raw_ranges {
            regions.push((
                "raw".into(),
                source.text[range.start..range.end].into(),
                range,
                true,
            ));
        }
    }
    if fields.contains(&SearchField::Comment) {
        for range in comment_ranges {
            regions.push((
                "comment".into(),
                source.text[range.start..range.end].into(),
                range,
                true,
            ));
        }
    }
    if fields.contains(&SearchField::Module) {
        regions.push((
            "module".into(),
            module.logical_path.to_string(),
            TextRange::new(0, 0),
            false,
        ));
    }
    if fields.contains(&SearchField::Path) {
        regions.push((
            "path".into(),
            relative_path(workspace.root(), &source.canonical_path)
                .to_string_lossy()
                .into_owned(),
            TextRange::new(0, 0),
            false,
        ));
    }
    if fields
        .iter()
        .any(|field| matches!(field, SearchField::Title | SearchField::Heading))
    {
        for (index, symbol) in symbols.into_iter().enumerate() {
            let field = if index == 0 && fields.contains(&SearchField::Title) {
                Some("title")
            } else if index > 0 && fields.contains(&SearchField::Heading) {
                Some("heading")
            } else {
                None
            };
            if let Some(field) = field {
                regions.push((field.into(), symbol.name, symbol.range, false));
            }
        }
    }
    if fields.contains(&SearchField::Id) {
        for label in workspace
            .labels()
            .iter()
            .filter(|label| label.file_id == source.file_id)
        {
            regions.push(("label".into(), label.name.clone(), label.range, false));
        }
    }
    if fields.contains(&SearchField::Tag)
        && let Some(parse) = &module.parse
    {
        for annotation in parse.annotations() {
            for attribute in &annotation.attributes.items {
                if let notist_syntax::Attribute::Tag(tag) = attribute {
                    regions.push(("tag".into(), tag.value.clone(), tag.range, false));
                }
            }
        }
    }
    regions
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SearchGroupKey {
    Source(String),
    Section(String, Option<usize>),
    Match(String, usize, usize),
}

fn search_group_key(
    workspace: &WorkspaceSnapshot,
    module: &notist_analysis::Module,
    range: TextRange,
    group_by: SearchGroup,
) -> SearchGroupKey {
    let module_name = module.logical_path.to_string();
    match group_by {
        SearchGroup::Source => SearchGroupKey::Source(module_name),
        SearchGroup::Section => {
            let heading = module.file_id.and_then(|file_id| {
                workspace
                    .document_symbols(file_id)
                    .into_iter()
                    .filter(|symbol| symbol.range.start <= range.start)
                    .max_by_key(|symbol| symbol.range.start)
                    .map(|symbol| symbol.range.start)
            });
            SearchGroupKey::Section(module_name, heading)
        }
        SearchGroup::Match => SearchGroupKey::Match(module_name, range.start, range.end),
    }
}

fn group_ranked_hits(
    workspace: &WorkspaceSnapshot,
    hits: Vec<SearchHit>,
    group_by: SearchGroup,
) -> Vec<SearchHit> {
    if group_by == SearchGroup::Match {
        return hits;
    }
    let mut seen = BTreeSet::new();
    hits.into_iter()
        .filter(|hit| {
            let Some(module_path) = parse_absolute_module_path(&hit.location.module) else {
                return true;
            };
            let Some(module) = workspace.module(&module_path) else {
                return true;
            };
            let range = TextRange::new(hit.location.byte_range.start, hit.location.byte_range.end);
            seen.insert(search_group_key(workspace, module, range, group_by))
        })
        .collect()
}

pub struct SearchIndex {
    reader: IndexReader,
    schema: SearchSchema,
    lexicons: HashMap<SearchField, Vec<String>>,
    pub stamp: IndexStamp,
    pub unit_count: usize,
}

enum IndexBuildPlan {
    Existing,
    Fresh,
    Incremental(HashMap<String, String>),
}

struct SearchSchema {
    title: Field,
    heading: Field,
    label: Field,
    module: Field,
    path: Field,
    tag: Field,
    body: Field,
    raw: Field,
    comment: Field,
    stored_module: Field,
    stored_path: Field,
    stored_start: Field,
    stored_end: Field,
    stored_kind: Field,
}

impl SearchIndex {
    pub fn remove_stored(root: &Path, source_fingerprint: &str) -> io::Result<()> {
        let Some(path) = search_cache_path(root, source_fingerprint) else {
            return Ok(());
        };
        if path.is_dir() {
            std::fs::remove_dir_all(path)?;
        }
        Ok(())
    }

    pub fn stored_status(root: &Path, source_fingerprint: &str) -> Option<IndexStatusRecord> {
        let path = search_cache_path(root, source_fingerprint)?;
        if !path.exists() {
            return None;
        }
        let manifest_path = path.join("notist-index.json");
        if !manifest_path.exists() {
            return Some(IndexStatusRecord {
                health: "building".into(),
                stamp: None,
                unit_count: 0,
                operation_handle: None,
                message: Some(
                    "the generation is not published; rebuild if this state persists".into(),
                ),
            });
        }
        let manifest = std::fs::read(&manifest_path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<JsonValue>(&bytes).map_err(|error| error.to_string())
            });
        let Ok(manifest) = manifest else {
            return Some(IndexStatusRecord {
                health: "error".into(),
                stamp: None,
                unit_count: 0,
                operation_handle: None,
                message: Some("the persisted index manifest is unreadable".into()),
            });
        };
        let vault_fingerprint = digest(root.to_string_lossy().as_bytes());
        let valid = manifest.get("schemaVersion").and_then(JsonValue::as_u64)
            == Some(u64::from(INDEX_SCHEMA_VERSION))
            && manifest.get("tokenizerVersion").and_then(JsonValue::as_str)
                == Some(TOKENIZER_VERSION)
            && manifest.get("rankingVersion").and_then(JsonValue::as_str) == Some(RANKING_VERSION)
            && manifest
                .get("sourceFingerprint")
                .and_then(JsonValue::as_str)
                == Some(source_fingerprint)
            && manifest
                .get("vaultRootFingerprint")
                .and_then(JsonValue::as_str)
                == Some(vault_fingerprint.as_str());
        if !valid {
            return Some(IndexStatusRecord {
                health: "stale".into(),
                stamp: None,
                unit_count: 0,
                operation_handle: None,
                message: Some("the persisted index manifest does not match this runtime".into()),
            });
        }
        Some(IndexStatusRecord {
            health: "ready".into(),
            stamp: Some(IndexStamp {
                source_fingerprint: source_fingerprint.into(),
                schema_version: manifest.get("schemaVersion")?.as_u64()? as u32,
                tokenizer_version: manifest.get("tokenizerVersion")?.as_str()?.into(),
                ranking_version: manifest.get("rankingVersion")?.as_str()?.into(),
            }),
            unit_count: usize::try_from(manifest.get("unitCount")?.as_u64()?).ok()?,
            operation_handle: None,
            message: Some("loaded from the persistent derived-index cache".into()),
        })
    }

    pub fn stale_stored_status(
        root: &Path,
        current_source_fingerprint: &str,
    ) -> Option<IndexStatusRecord> {
        let current = search_cache_path(root, current_source_fingerprint)?;
        let mut generations = std::fs::read_dir(current.parent()?)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir() && path != &current)
            .collect::<Vec<_>>();
        generations.sort();
        for generation in generations.into_iter().rev() {
            let Some(fingerprint) = generation.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(mut status) = Self::stored_status(root, fingerprint) else {
                continue;
            };
            if status.health == "ready" {
                status.health = "stale".into();
                status.message = Some("the persisted index belongs to an older source set".into());
                return Some(status);
            }
        }
        None
    }

    pub fn build(workspace: &WorkspaceSnapshot, source_fingerprint: &str) -> io::Result<Self> {
        let (schema, search_schema) = make_search_schema();
        let persistent = workspace
            .sources()
            .all(|source| matches!(source.origin, notist_analysis::SourceOrigin::Disk));
        let cache = persistent
            .then(|| search_cache_path(workspace.root(), source_fingerprint))
            .flatten();
        let (index, plan) = if let Some(path) = &cache {
            std::fs::create_dir_all(path.parent().unwrap())?;
            if path.join("notist-index.json").is_file() {
                let status =
                    Self::stored_status(workspace.root(), source_fingerprint).ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "missing index status")
                    })?;
                if status.health != "ready" {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        status
                            .message
                            .unwrap_or_else(|| "index manifest is invalid".into()),
                    ));
                }
                (
                    Index::open_in_dir(path).map_err(io::Error::other)?,
                    IndexBuildPlan::Existing,
                )
            } else {
                if path.exists() {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "another process is building this index generation; rebuild if it remains incomplete",
                    ));
                }
                if let Some(previous) = previous_index_generation(path) {
                    copy_index_generation(&previous.0, path)?;
                    (
                        Index::open_in_dir(path).map_err(io::Error::other)?,
                        IndexBuildPlan::Incremental(previous.1),
                    )
                } else {
                    std::fs::create_dir(path)?;
                    (
                        Index::create_in_dir(path, schema.clone()).map_err(io::Error::other)?,
                        IndexBuildPlan::Fresh,
                    )
                }
            }
        } else {
            (Index::create_in_ram(schema), IndexBuildPlan::Fresh)
        };
        let publish = !matches!(plan, IndexBuildPlan::Existing);
        match plan {
            IndexBuildPlan::Existing => {}
            IndexBuildPlan::Fresh => {
                populate_index(&index, &search_schema, workspace)?;
            }
            IndexBuildPlan::Incremental(previous_sources) => {
                update_index(&index, &search_schema, workspace, &previous_sources)?;
            }
        }
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(io::Error::other)?;
        let count = usize::try_from(reader.searcher().num_docs()).unwrap_or(usize::MAX);
        if publish && let Some(path) = &cache {
            let manifest = serde_json::json!({
                "schemaVersion": INDEX_SCHEMA_VERSION,
                "tokenizerVersion": TOKENIZER_VERSION,
                "rankingVersion": RANKING_VERSION,
                "sourceFingerprint": source_fingerprint,
                "vaultRootFingerprint": digest(workspace.root().to_string_lossy().as_bytes()),
                "unitCount": count,
                "sources": workspace.sources().map(|source| serde_json::json!({
                    "path": relative_path(workspace.root(), &source.canonical_path),
                    "fingerprint": fingerprint(&source.text),
                })).collect::<Vec<_>>(),
            });
            crate::request::write_artifact_atomic(
                &path.join("notist-index.json"),
                &serde_json::to_vec(&manifest)?,
                "index-manifest",
            )?;
        }
        Ok(Self {
            reader,
            schema: search_schema,
            lexicons: build_lexicons(workspace),
            stamp: IndexStamp {
                source_fingerprint: source_fingerprint.into(),
                schema_version: INDEX_SCHEMA_VERSION,
                tokenizer_version: TOKENIZER_VERSION.into(),
                ranking_version: RANKING_VERSION.into(),
            },
            unit_count: count,
        })
    }

    pub fn search(
        &self,
        workspace: &WorkspaceSnapshot,
        snapshot: &SnapshotIdentity,
        request: &SearchQuery,
    ) -> Result<QueryPage<SearchHit>, ToolError> {
        validate_search(request)?;
        if self.stamp.source_fingerprint != snapshot.source_fingerprint {
            return Err(ToolError::new(
                "index_not_ready",
                "search index does not match the captured snapshot",
            )
            .retryable("retry, inspect index status, or use --exact"));
        }
        let term_groups = query_term_groups(&request.query);
        if term_groups.is_empty() {
            return Err(ToolError::new(
                "invalid_argument",
                "query has no searchable terms",
            ));
        }
        let fields = if request.fields.is_empty() {
            SearchField::defaults()
        } else {
            request.fields.clone()
        };
        let mut groups: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        let mut expansion_count = 0usize;
        let mut expansion_limited = false;
        for term_group in term_groups {
            let mut variants: Vec<(Occur, Box<dyn Query>)> = Vec::new();
            for term_text in term_group {
                for field_kind in &fields {
                    let Some((field, boost)) = self.field(*field_kind) else {
                        continue;
                    };
                    let expansions =
                        if request.mode == SearchMode::Fuzzy && fuzzy_eligible(&term_text) {
                            let remaining = 128usize.saturating_sub(expansion_count).min(32);
                            let (terms, limited) = self.fuzzy_terms(
                                *field_kind,
                                &term_text,
                                request.fuzzy_distance,
                                remaining,
                            );
                            expansion_limited |= limited || remaining == 0;
                            expansion_count += terms.len();
                            terms
                        } else {
                            vec![(term_text.clone(), 0)]
                        };
                    for (expanded, distance) in expansions {
                        let term = Term::from_field_text(field, &expanded);
                        let query: Box<dyn Query> = Box::new(TermQuery::new(
                            term,
                            IndexRecordOption::WithFreqsAndPositions,
                        ));
                        let penalty = 1.0 / (f32::from(distance) + 1.0);
                        variants.push((
                            Occur::Should,
                            Box::new(BoostQuery::new(query, boost * penalty)),
                        ));
                    }
                }
            }
            if !variants.is_empty() {
                groups.push((
                    if request.operator == SearchOperator::All {
                        Occur::Must
                    } else {
                        Occur::Should
                    },
                    Box::new(BooleanQuery::new(variants)),
                ));
            }
        }
        if !request.scopes.is_empty() {
            let mut scopes: Vec<(Occur, Box<dyn Query>)> = Vec::new();
            for scope in &request.scopes {
                let pattern = format!("{}(::.*)?", regex::escape(scope));
                let query = RegexQuery::from_pattern(&pattern, self.schema.stored_module)
                    .map_err(|error| ToolError::new("invalid_argument", error.to_string()))?;
                scopes.push((Occur::Should, Box::new(query)));
            }
            groups.push((
                Occur::Must,
                Box::new(ConstScoreQuery::new(
                    Box::new(BooleanQuery::new(scopes)),
                    0.0,
                )),
            ));
        }
        let query = BooleanQuery::new(groups);
        let searcher = self.reader.searcher();
        let (top, total) = searcher
            .search(
                &query,
                &(TopDocs::with_limit(10_000).order_by_score(), Count),
            )
            .map_err(|error| ToolError::new("search_failed", error.to_string()))?;
        let mut hits = Vec::new();
        let mut seen = BTreeSet::new();
        for (score, address) in top {
            let document: TantivyDocument = searcher
                .doc(address)
                .map_err(|error| ToolError::new("search_failed", error.to_string()))?;
            let module_name = text_value(&document, self.schema.stored_module).unwrap_or_default();
            if !in_scope(&module_name, &request.scopes) {
                continue;
            }
            let path =
                PathBuf::from(text_value(&document, self.schema.stored_path).unwrap_or_default());
            let start = u64_value(&document, self.schema.stored_start).unwrap_or(0) as usize;
            let end = u64_value(&document, self.schema.stored_end).unwrap_or(start as u64) as usize;
            if !seen.insert((module_name.clone(), start, end)) {
                continue;
            }
            let Some(module_path) = parse_absolute_module_path(&module_name) else {
                continue;
            };
            let Some(module) = workspace.module(&module_path) else {
                continue;
            };
            let Some(source) = module.file_id.and_then(|file_id| workspace.source(file_id)) else {
                continue;
            };
            if end > source.text.len() || start > end {
                continue;
            }
            let unit = TextRange::new(start, end);
            let matched = lexical_match_range(&source.text, unit, &request.query);
            let (excerpt_text, excerpt_range, truncated) =
                excerpt(&source.text, matched.unwrap_or(unit), request.snippet_bytes);
            let kind =
                text_value(&document, self.schema.stored_kind).unwrap_or_else(|| "body".into());
            hits.push(SearchHit {
                location: location(workspace, module, source, unit, None),
                matched_field: kind,
                match_range: matched.map(Into::into),
                unit_range: unit.into(),
                excerpt: excerpt_text,
                excerpt_range: excerpt_range.into(),
                excerpt_truncated: truncated,
                score: Some((score.max(0.0) * 1_000_000.0).round() as u64),
            });
            let _ = path;
        }
        hits.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then(left.location.module.cmp(&right.location.module))
                .then(
                    left.location
                        .byte_range
                        .start
                        .cmp(&right.location.byte_range.start),
                )
        });
        let group_by = request.applied_group_by();
        let hits = group_ranked_hits(workspace, hits, group_by);
        let mut page = page(
            snapshot,
            "search",
            &search_fingerprint(request),
            &request.page,
            SEARCH_DEFAULT_LIMIT,
            hits,
        )?;
        page.search = Some(SearchPageMetadata {
            group_by,
            ordering: "relevance".into(),
            ranking_version: Some(RANKING_VERSION.into()),
            index_stamp: Some(self.stamp.clone()),
            expansion_limited,
        });
        if !page.page.has_more && total > 10_000 {
            page.coverage.complete = false;
            page.coverage.stop_reason = "query_limit".into();
        }
        add_empty_search_hint(&mut page, request);
        Ok(page)
    }

    fn field(&self, field: SearchField) -> Option<(Field, f32)> {
        Some(match field {
            SearchField::Title => (self.schema.title, 5.0),
            SearchField::Heading => (self.schema.heading, 4.0),
            SearchField::Id => (self.schema.label, 6.0),
            SearchField::Module => (self.schema.module, 6.0),
            SearchField::Path => (self.schema.path, 3.0),
            SearchField::Tag => (self.schema.tag, 4.0),
            SearchField::Body => (self.schema.body, 1.0),
            SearchField::Raw => (self.schema.raw, 0.7),
            SearchField::Comment => (self.schema.comment, 0.5),
        })
    }

    fn fuzzy_terms(
        &self,
        field: SearchField,
        query: &str,
        max_distance: u8,
        limit: usize,
    ) -> (Vec<(String, u8)>, bool) {
        let mut candidates = self
            .lexicons
            .get(&field)
            .into_iter()
            .flatten()
            .filter_map(|term| {
                let distance = damerau_levenshtein(query, term);
                (distance <= usize::from(max_distance)).then_some((term.clone(), distance as u8))
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.1.cmp(&right.1).then(left.0.cmp(&right.0)));
        let limited = candidates.len() > limit;
        candidates.truncate(limit);
        (candidates, limited)
    }
}

fn make_search_schema() -> (Schema, SearchSchema) {
    let mut builder = Schema::builder();
    let title = builder.add_text_field("title", TEXT);
    let heading = builder.add_text_field("heading", TEXT);
    let label = builder.add_text_field("id", TEXT);
    let module = builder.add_text_field("module", TEXT);
    let path = builder.add_text_field("path", TEXT);
    let tag = builder.add_text_field("tag", TEXT);
    let body = builder.add_text_field("body", TEXT);
    let raw = builder.add_text_field("raw", TEXT);
    let comment = builder.add_text_field("comment", TEXT);
    let stored_module = builder.add_text_field("stored_module", STRING | STORED);
    let stored_path = builder.add_text_field("stored_path", STRING | STORED);
    let stored_start = builder.add_u64_field("stored_start", STORED);
    let stored_end = builder.add_u64_field("stored_end", STORED);
    let stored_kind = builder.add_text_field("stored_kind", STRING | STORED);
    let schema = builder.build();
    (
        schema,
        SearchSchema {
            title,
            heading,
            label,
            module,
            path,
            tag,
            body,
            raw,
            comment,
            stored_module,
            stored_path,
            stored_start,
            stored_end,
            stored_kind,
        },
    )
}

fn previous_index_generation(current: &Path) -> Option<(PathBuf, HashMap<String, String>)> {
    let parent = current.parent()?;
    let mut candidates = std::fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path != current)
        .collect::<Vec<_>>();
    candidates.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    for path in candidates.into_iter().rev() {
        let Some(manifest) = std::fs::read(path.join("notist-index.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<JsonValue>(&bytes).ok())
        else {
            continue;
        };
        let compatible = manifest.get("schemaVersion").and_then(JsonValue::as_u64)
            == Some(u64::from(INDEX_SCHEMA_VERSION))
            && manifest.get("tokenizerVersion").and_then(JsonValue::as_str)
                == Some(TOKENIZER_VERSION)
            && manifest.get("rankingVersion").and_then(JsonValue::as_str) == Some(RANKING_VERSION);
        if !compatible {
            continue;
        }
        let Some(source_items) = manifest.get("sources").and_then(JsonValue::as_array) else {
            continue;
        };
        let sources = source_items
            .iter()
            .map(|source| {
                Some((
                    source.get("path")?.as_str()?.to_owned(),
                    source.get("fingerprint")?.as_str()?.to_owned(),
                ))
            })
            .collect::<Option<HashMap<_, _>>>();
        let Some(sources) = sources else {
            continue;
        };
        return Some((path, sources));
    }
    None
}

fn copy_index_generation(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if name_text == "notist-index.json"
            || name_text.ends_with(".lock")
            || name_text.contains(".notist-")
        {
            continue;
        }
        let source = entry.path();
        let target = to.join(name);
        if source.is_dir() {
            copy_index_generation(&source, &target)?;
        } else {
            std::fs::copy(source, target)?;
        }
    }
    Ok(())
}

fn update_index(
    index: &Index,
    fields: &SearchSchema,
    workspace: &WorkspaceSnapshot,
    previous_sources: &HashMap<String, String>,
) -> io::Result<()> {
    let current_sources = workspace
        .sources()
        .map(|source| {
            (
                relative_path(workspace.root(), &source.canonical_path)
                    .to_string_lossy()
                    .into_owned(),
                fingerprint(&source.text),
            )
        })
        .collect::<HashMap<_, _>>();
    let changed = current_sources
        .iter()
        .filter(|entry| previous_sources.get(entry.0) != Some(entry.1))
        .map(|(path, _)| path.clone())
        .collect::<BTreeSet<_>>();
    let removed = previous_sources
        .keys()
        .filter(|path| !current_sources.contains_key(*path))
        .cloned()
        .collect::<BTreeSet<_>>();
    if !changed.is_empty() || !removed.is_empty() {
        let mut writer = index
            .writer::<TantivyDocument>(20_000_000)
            .map_err(io::Error::other)?;
        for path in changed.iter().chain(&removed) {
            writer.delete_term(Term::from_field_text(fields.stored_path, path));
        }
        writer.commit().map_err(io::Error::other)?;
    }
    if !changed.is_empty() {
        populate_index_paths(index, fields, workspace, Some(&changed))?;
    }
    Ok(())
}

fn populate_index(
    index: &Index,
    fields: &SearchSchema,
    workspace: &WorkspaceSnapshot,
) -> io::Result<usize> {
    populate_index_paths(index, fields, workspace, None)
}

fn populate_index_paths(
    index: &Index,
    fields: &SearchSchema,
    workspace: &WorkspaceSnapshot,
    included_paths: Option<&BTreeSet<String>>,
) -> io::Result<usize> {
    let mut writer = index.writer(20_000_000).map_err(io::Error::other)?;
    let mut count = 0usize;
    for module_record in workspace
        .modules()
        .filter(|module| module.file_id.is_some())
    {
        let source = workspace.source(module_record.file_id.unwrap()).unwrap();
        let relative = relative_path(workspace.root(), &source.canonical_path);
        if included_paths.is_some_and(|paths| !paths.contains(relative.to_string_lossy().as_ref()))
        {
            continue;
        }
        let symbols = workspace.document_symbols(source.file_id);
        let raw_ranges = module_record
            .parse
            .as_ref()
            .map(|parse| {
                parse
                    .raw_literals()
                    .into_iter()
                    .map(|raw| raw.payload_range)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let comment_ranges = comment_ranges(&source.text);
        let mut excluded = raw_ranges.clone();
        excluded.extend(comment_ranges.iter().copied());
        excluded.extend(symbols.iter().map(|symbol| symbol.range));
        let excluded = merge_ranges(excluded);
        writer
            .add_document(doc!(
                fields.module => normalize_for_index(&module_record.logical_path.to_string()),
                fields.stored_module => module_record.logical_path.to_string(),
                fields.stored_path => relative.to_string_lossy().to_string(),
                fields.stored_start => 0u64,
                fields.stored_end => 0u64,
                fields.stored_kind => "module",
            ))
            .map_err(io::Error::other)?;
        count += 1;
        writer
            .add_document(doc!(
                fields.path => normalize_for_index(&relative.to_string_lossy()),
                fields.stored_module => module_record.logical_path.to_string(),
                fields.stored_path => relative.to_string_lossy().to_string(),
                fields.stored_start => 0u64,
                fields.stored_end => 0u64,
                fields.stored_kind => "path",
            ))
            .map_err(io::Error::other)?;
        count += 1;
        for (index, symbol) in symbols.iter().enumerate() {
            let mut document = TantivyDocument::default();
            let kind = if index == 0 { "title" } else { "heading" };
            document.add_text(
                if index == 0 {
                    fields.title
                } else {
                    fields.heading
                },
                normalize_for_index(&symbol.name),
            );
            document.add_text(fields.stored_module, module_record.logical_path.to_string());
            document.add_text(fields.stored_path, relative.to_string_lossy());
            document.add_u64(fields.stored_start, symbol.range.start as u64);
            document.add_u64(fields.stored_end, symbol.range.end as u64);
            document.add_text(fields.stored_kind, kind);
            writer.add_document(document).map_err(io::Error::other)?;
            count += 1;
        }
        for range in semantic_units(&source.text) {
            writer
                .add_document(doc!(
                    fields.body => normalize_for_index(&text_excluding(&source.text, range, &excluded)),
                    fields.stored_module => module_record.logical_path.to_string(),
                    fields.stored_path => relative.to_string_lossy().to_string(),
                    fields.stored_start => range.start as u64,
                    fields.stored_end => range.end as u64,
                    fields.stored_kind => "body",
                ))
                .map_err(io::Error::other)?;
            count += 1;
        }
        for definition in workspace
            .labels()
            .iter()
            .filter(|label| label.file_id == source.file_id)
        {
            writer
                .add_document(doc!(
                    fields.label => normalize_for_index(&definition.name),
                    fields.stored_module => module_record.logical_path.to_string(),
                    fields.stored_path => relative.to_string_lossy().to_string(),
                    fields.stored_start => definition.range.start as u64,
                    fields.stored_end => definition.range.end as u64,
                    fields.stored_kind => "id",
                ))
                .map_err(io::Error::other)?;
            count += 1;
        }
        // Heading default ids participate in the `id` field (D0008 field table).
        {
            for (name, range) in workspace.module_heading_default_ids(&module_record.logical_path) {
                writer
                    .add_document(doc!(
                        fields.label => normalize_for_index(&name),
                        fields.stored_module => module_record.logical_path.to_string(),
                        fields.stored_path => relative.to_string_lossy().to_string(),
                        fields.stored_start => range.start as u64,
                        fields.stored_end => range.end as u64,
                        fields.stored_kind => "id",
                    ))
                    .map_err(io::Error::other)?;
                count += 1;
            }
        }
        if let Some(parse) = &module_record.parse {
            for literal in parse.raw_literals() {
                let range = literal.payload_range;
                writer
                    .add_document(doc!(
                        fields.raw => normalize_for_index(&source.text[range.start..range.end]),
                        fields.stored_module => module_record.logical_path.to_string(),
                        fields.stored_path => relative.to_string_lossy().to_string(),
                        fields.stored_start => range.start as u64,
                        fields.stored_end => range.end as u64,
                        fields.stored_kind => "raw",
                    ))
                    .map_err(io::Error::other)?;
                count += 1;
            }
            for annotation in parse.annotations() {
                for attribute in &annotation.attributes.items {
                    if let notist_syntax::Attribute::Tag(tag_name) = attribute {
                        writer
                            .add_document(doc!(
                                fields.tag => normalize_for_index(&tag_name.value),
                                fields.stored_module => module_record.logical_path.to_string(),
                                fields.stored_path => relative.to_string_lossy().to_string(),
                                fields.stored_start => tag_name.range.start as u64,
                                fields.stored_end => tag_name.range.end as u64,
                                fields.stored_kind => "tag",
                            ))
                            .map_err(io::Error::other)?;
                        count += 1;
                    }
                }
            }
        }
        for range in comment_ranges {
            writer
                .add_document(doc!(
                    fields.comment => normalize_for_index(&source.text[range.start..range.end]),
                    fields.stored_module => module_record.logical_path.to_string(),
                    fields.stored_path => relative.to_string_lossy().to_string(),
                    fields.stored_start => range.start as u64,
                    fields.stored_end => range.end as u64,
                    fields.stored_kind => "comment",
                ))
                .map_err(io::Error::other)?;
            count += 1;
        }
    }
    writer.commit().map_err(io::Error::other)?;
    Ok(count)
}

fn search_cache_path(root: &Path, source_fingerprint: &str) -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        Some(PathBuf::from(xdg))
    } else {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache"))
    }?;
    let vault = digest(root.to_string_lossy().as_bytes());
    Some(
        base.join("Notist")
            .join("indexes")
            .join(&vault[..16])
            .join(format!(
                "schema-{INDEX_SCHEMA_VERSION}-{TOKENIZER_VERSION}-{RANKING_VERSION}"
            ))
            .join(source_fingerprint),
    )
}

fn build_lexicons(workspace: &WorkspaceSnapshot) -> HashMap<SearchField, Vec<String>> {
    let mut values: HashMap<SearchField, BTreeSet<String>> = HashMap::new();
    let mut add = |field: SearchField, text: &str| {
        values.entry(field).or_default().extend(query_terms(text));
    };
    for module in workspace
        .modules()
        .filter(|module| module.file_id.is_some())
    {
        let source = workspace.source(module.file_id.unwrap()).unwrap();
        let relative = relative_path(workspace.root(), &source.canonical_path);
        add(SearchField::Module, &module.logical_path.to_string());
        add(SearchField::Path, &relative.to_string_lossy());
        let raw_ranges = module
            .parse
            .as_ref()
            .map(|parse| {
                parse
                    .raw_literals()
                    .into_iter()
                    .map(|raw| raw.payload_range)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let comments = comment_ranges(&source.text);
        let symbols = workspace.document_symbols(source.file_id);
        let mut excluded = raw_ranges;
        excluded.extend(comments.iter().copied());
        excluded.extend(symbols.iter().map(|symbol| symbol.range));
        add(
            SearchField::Body,
            &text_excluding(
                &source.text,
                TextRange::new(0, source.text.len()),
                &merge_ranges(excluded),
            ),
        );
        for range in comments {
            add(SearchField::Comment, &source.text[range.start..range.end]);
        }
        for (index, symbol) in symbols.into_iter().enumerate() {
            add(
                if index == 0 {
                    SearchField::Title
                } else {
                    SearchField::Heading
                },
                &symbol.name,
            );
        }
        for label in workspace
            .labels()
            .iter()
            .filter(|label| label.file_id == source.file_id)
        {
            add(SearchField::Id, &label.name);
        }
        if let Some(parse) = &module.parse {
            for literal in parse.raw_literals() {
                add(
                    SearchField::Raw,
                    &source.text[literal.payload_range.start..literal.payload_range.end],
                );
            }
            for annotation in parse.annotations() {
                for attribute in &annotation.attributes.items {
                    if let notist_syntax::Attribute::Tag(tag) = attribute {
                        add(SearchField::Tag, &tag.value);
                    }
                }
            }
        }
    }
    values
        .into_iter()
        .map(|(field, terms)| (field, terms.into_iter().collect()))
        .collect()
}

fn damerau_levenshtein(left: &str, right: &str) -> usize {
    let left = left.chars().collect::<Vec<_>>();
    let right = right.chars().collect::<Vec<_>>();
    let mut table = vec![vec![0usize; right.len() + 1]; left.len() + 1];
    for (index, row) in table.iter_mut().enumerate() {
        row[0] = index;
    }
    for index in 0..=right.len() {
        table[0][index] = index;
    }
    for i in 1..=left.len() {
        for j in 1..=right.len() {
            let cost = usize::from(left[i - 1] != right[j - 1]);
            table[i][j] = (table[i - 1][j] + 1)
                .min(table[i][j - 1] + 1)
                .min(table[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && left[i - 1] == right[j - 2] && left[i - 2] == right[j - 1] {
                table[i][j] = table[i][j].min(table[i - 2][j - 2] + 1);
            }
        }
    }
    table[left.len()][right.len()]
}

fn text_value(document: &TantivyDocument, field: Field) -> Option<String> {
    document.get_first(field)?.as_str().map(str::to_owned)
}

fn u64_value(document: &TantivyDocument, field: Field) -> Option<u64> {
    document.get_first(field)?.as_u64()
}

fn validate_search(query: &SearchQuery) -> Result<(), ToolError> {
    if query.query.is_empty() || query.query.len() > 4096 {
        return Err(ToolError::new(
            "invalid_argument",
            "query must contain 1 to 4096 UTF-8 bytes",
        ));
    }
    validate_snippet(query.snippet_bytes)?;
    if query.scopes.len() > 32
        || query
            .scopes
            .iter()
            .any(|scope| scope.len() > 4096 || parse_absolute_module_path(scope).is_none())
    {
        return Err(ToolError::new(
            "invalid_argument",
            "scope must contain at most 32 absolute ModulePath prefixes of at most 4096 bytes",
        ));
    }
    if query.mode == SearchMode::Fuzzy && !(1..=2).contains(&query.fuzzy_distance) {
        return Err(ToolError::new(
            "invalid_argument",
            "fuzzy distance must be 1 or 2",
        ));
    }
    if query.mode != SearchMode::Fuzzy && query.fuzzy_distance != 1 {
        return Err(ToolError::new(
            "invalid_argument",
            "fuzzy-distance is only valid in fuzzy mode",
        ));
    }
    if !matches!(query.mode, SearchMode::Exact | SearchMode::Regex) && query.ignore_case {
        return Err(ToolError::new(
            "invalid_argument",
            "ignore-case is only valid in exact or regex mode",
        ));
    }
    if query.wait_index_ms > 10_000 {
        return Err(ToolError::new(
            "invalid_argument",
            "wait-index may not exceed 10 seconds",
        ));
    }
    if matches!(query.mode, SearchMode::Exact | SearchMode::Regex) && query.wait_index_ms != 2000 {
        return Err(ToolError::new(
            "invalid_argument",
            "wait-index is only valid in lexical or fuzzy mode",
        ));
    }
    Ok(())
}

fn validate_snippet(bytes: usize) -> Result<(), ToolError> {
    if !(64..=MAX_SNIPPET_BYTES).contains(&bytes) {
        return Err(ToolError::new(
            "invalid_argument",
            "snippet bytes must be between 64 and 2048",
        ));
    }
    Ok(())
}

fn page<T: Clone + Serialize + DeserializeOwned>(
    snapshot: &SnapshotIdentity,
    operation: &str,
    query_fingerprint: &str,
    request: &PageRequest,
    default_limit: usize,
    all_items: Vec<T>,
) -> Result<QueryPage<T>, ToolError> {
    let requested_limit = request.limit.unwrap_or(default_limit);
    if requested_limit == 0 || requested_limit > HARD_LIMIT {
        return Err(ToolError::new(
            "budget_too_large",
            format!("limit must be between 1 and {HARD_LIMIT}"),
        ));
    }
    let max_bytes = applied_max_bytes(request)?;
    let offset = cursor_offset(snapshot, operation, query_fingerprint, request)?.unwrap_or(0);
    if offset > all_items.len() {
        return Err(
            ToolError::new("invalid_cursor", "cursor offset is outside the result set")
                .with_hint("omit cursor to restart the query from the first page"),
        );
    }
    let mut items = Vec::new();
    let mut exhausted = false;
    for item in all_items.iter().skip(offset).take(requested_limit) {
        let mut candidate = items.clone();
        candidate.push(item.clone());
        if serde_json::to_vec(&candidate)
            .map(|value| value.len() + 2048)
            .unwrap_or(max_bytes + 1)
            > max_bytes
        {
            exhausted = true;
            break;
        }
        items.push(item.clone());
    }
    let next_offset = offset + items.len();
    let has_more = next_offset < all_items.len();
    let next_cursor =
        has_more.then(|| encode_cursor(operation, snapshot, query_fingerprint, next_offset));
    let logical_bytes = serde_json::to_vec(&items)
        .map(|value| value.len() + 1024)
        .unwrap_or(0);
    let stop_reason = if !has_more {
        "complete"
    } else if exhausted {
        "byte_budget"
    } else {
        "item_limit"
    };
    Ok(QueryPage {
        snapshot: snapshot.clone(),
        page: PageInfo {
            requested_limit,
            applied_limit: requested_limit,
            returned: items.len(),
            has_more,
            next_cursor,
        },
        budget: BudgetInfo {
            requested_bytes: request.max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
            applied_bytes: max_bytes,
            logical_bytes,
            exhausted,
        },
        coverage: CoverageInfo {
            complete: !has_more,
            stop_reason: stop_reason.into(),
        },
        search: None,
        hints: continuation_hints(has_more),
        items,
    })
}

fn continuation_hints(has_more: bool) -> Vec<String> {
    has_more
        .then(|| {
            "more results: repeat the same query with next_cursor; only page limits may change"
                .into()
        })
        .into_iter()
        .collect()
}

fn applied_max_bytes(request: &PageRequest) -> Result<usize, ToolError> {
    let requested = request.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
    if !(MIN_MAX_BYTES..=HARD_MAX_BYTES).contains(&requested) {
        return Err(ToolError::new(
            "budget_too_large",
            format!("max-bytes must be between {MIN_MAX_BYTES} and {HARD_MAX_BYTES}"),
        ));
    }
    if request
        .cursor
        .as_ref()
        .is_some_and(|cursor| cursor.len() > CURSOR_MAX_BYTES)
    {
        return Err(
            ToolError::new("invalid_cursor", "cursor exceeds 4096 bytes")
                .with_hint("omit cursor and restart the query"),
        );
    }
    Ok(requested)
}

fn cursor_offset(
    snapshot: &SnapshotIdentity,
    operation: &str,
    query_fingerprint: &str,
    request: &PageRequest,
) -> Result<Option<usize>, ToolError> {
    let Some(cursor) = &request.cursor else {
        return Ok(None);
    };
    let payload = decode_cursor(cursor)?;
    if payload.operation != operation
        || payload.query_fingerprint != digest(query_fingerprint.as_bytes())
    {
        return Err(ToolError::new(
            "invalid_cursor",
            "cursor does not belong to this query",
        )
        .with_hint(
            "cursor is bound to the original selector, query, filters, mode, grouping, and ordering; resend those parameters unchanged with cursor, or omit cursor to restart",
        ));
    }
    if payload.vault_fingerprint != snapshot.vault.fingerprint
        || payload.view_kind != snapshot.view_kind
        || (!payload.daemon_instance.is_empty()
            && payload.daemon_instance != snapshot.daemon_instance.0)
        || (payload.view_id != 0 && payload.view_id != snapshot.view_id.0)
    {
        return Err(ToolError::new(
            "invalid_cursor",
            "cursor belongs to another Vault, daemon instance, or view",
        )
        .with_hint(
            "use the cursor only with the Vault and view that issued it, or omit cursor to restart",
        ));
    }
    if payload.source_fingerprint != snapshot.source_fingerprint {
        return Err(ToolError::new(
            "cursor_stale",
            "Vault sources changed after the cursor was issued",
        )
        .retryable("restart the query without a cursor"));
    }
    Ok(Some(payload.offset))
}

fn encode_cursor(
    operation: &str,
    snapshot: &SnapshotIdentity,
    query: &str,
    offset: usize,
) -> String {
    let Some(vault_fingerprint) = decode_hex_fixed::<8>(&snapshot.vault.fingerprint) else {
        return encode_legacy_cursor(operation, snapshot, query, offset);
    };
    let Some(source_fingerprint) = decode_hex_fixed::<8>(&snapshot.source_fingerprint) else {
        return encode_legacy_cursor(operation, snapshot, query, offset);
    };
    let query_fingerprint = digest(query.as_bytes());
    let Some(query_fingerprint) = decode_hex_fixed::<32>(&query_fingerprint) else {
        return encode_legacy_cursor(operation, snapshot, query, offset);
    };
    let Ok(operation_length) = u8::try_from(operation.len()) else {
        return encode_legacy_cursor(operation, snapshot, query, offset);
    };
    let Ok(view_length) = u8::try_from(snapshot.view_kind.len()) else {
        return encode_legacy_cursor(operation, snapshot, query, offset);
    };
    let Ok(instance_length) = u8::try_from(snapshot.daemon_instance.0.len()) else {
        return encode_legacy_cursor(operation, snapshot, query, offset);
    };
    let mut bytes = Vec::with_capacity(
        84 + operation.len()
            + snapshot.view_kind.len()
            + snapshot.daemon_instance.0.len(),
    );
    bytes.extend([3, operation_length]);
    bytes.extend(operation.as_bytes());
    bytes.push(view_length);
    bytes.extend(snapshot.view_kind.as_bytes());
    bytes.push(instance_length);
    bytes.extend(snapshot.daemon_instance.0.as_bytes());
    bytes.extend(snapshot.view_id.0.to_le_bytes());
    bytes.extend(vault_fingerprint);
    bytes.extend(source_fingerprint);
    bytes.extend(query_fingerprint);
    bytes.extend((offset as u64).to_le_bytes());
    let checksum = Sha256::digest(&bytes);
    bytes.extend(&checksum[..8]);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn encode_legacy_cursor(
    operation: &str,
    snapshot: &SnapshotIdentity,
    query: &str,
    offset: usize,
) -> String {
    let payload = CursorPayload {
        version: 1,
        operation: operation.into(),
        vault_fingerprint: snapshot.vault.fingerprint.clone(),
        view_kind: snapshot.view_kind.clone(),
        daemon_instance: snapshot.daemon_instance.0.clone(),
        view_id: snapshot.view_id.0,
        source_fingerprint: snapshot.source_fingerprint.clone(),
        query_fingerprint: digest(query.as_bytes()),
        offset,
    };
    let bytes = serde_json::to_vec(&payload).unwrap();
    let checksum = digest(&bytes);
    format!("{}.{}", URL_SAFE_NO_PAD.encode(bytes), &checksum[..16])
}

fn decode_cursor(cursor: &str) -> Result<CursorPayload, ToolError> {
    if !cursor.contains('.') {
        return decode_packed_cursor(cursor);
    }
    decode_legacy_cursor(cursor)
}

fn decode_packed_cursor(cursor: &str) -> Result<CursorPayload, ToolError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| invalid_cursor("cursor is not valid base64url"))?;
    if bytes.len() < 1 + 1 + 1 + 1 + 8 + 8 + 8 + 32 + 8 + 8 {
        return Err(invalid_cursor("cursor payload is too short"));
    }
    let (payload, checksum) = bytes.split_at(bytes.len() - 8);
    if Sha256::digest(payload)[..8] != checksum[..] {
        return Err(invalid_cursor("cursor checksum does not match"));
    }
    let mut index = 0usize;
    let version = take_byte(payload, &mut index)?;
    if version != 3 {
        return Err(invalid_cursor("cursor schema is unsupported"));
    }
    let operation = take_string(payload, &mut index)?;
    let view_kind = take_string(payload, &mut index)?;
    let daemon_instance = take_string(payload, &mut index)?;
    let view_id_bytes: [u8; 8] = take_bytes(payload, &mut index, 8)?
        .try_into()
        .map_err(|_| invalid_cursor("cursor view id is malformed"))?;
    let view_id = u64::from_le_bytes(view_id_bytes);
    let vault_fingerprint = encode_hex(take_bytes(payload, &mut index, 8)?);
    let source_fingerprint = encode_hex(take_bytes(payload, &mut index, 8)?);
    let query_fingerprint = encode_hex(take_bytes(payload, &mut index, 32)?);
    let offset_bytes: [u8; 8] = take_bytes(payload, &mut index, 8)?
        .try_into()
        .map_err(|_| invalid_cursor("cursor offset is malformed"))?;
    if index != payload.len() {
        return Err(invalid_cursor("cursor payload has trailing data"));
    }
    let offset = usize::try_from(u64::from_le_bytes(offset_bytes))
        .map_err(|_| invalid_cursor("cursor offset is too large"))?;
    Ok(CursorPayload {
        version: 3,
        operation,
        vault_fingerprint,
        view_kind,
        daemon_instance,
        view_id,
        source_fingerprint,
        query_fingerprint,
        offset,
    })
}

fn decode_legacy_cursor(cursor: &str) -> Result<CursorPayload, ToolError> {
    let (payload, checksum) = cursor
        .split_once('.')
        .ok_or_else(|| invalid_cursor("cursor is malformed"))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| invalid_cursor("cursor is not valid base64url"))?;
    if !digest(&bytes).starts_with(checksum) {
        return Err(invalid_cursor("cursor checksum does not match"));
    }
    let payload: CursorPayload =
        serde_json::from_slice(&bytes).map_err(|_| invalid_cursor("cursor payload is invalid"))?;
    if payload.version != 1 {
        return Err(invalid_cursor("cursor schema is unsupported"));
    }
    Ok(payload)
}

fn take_byte(bytes: &[u8], index: &mut usize) -> Result<u8, ToolError> {
    let value = *bytes
        .get(*index)
        .ok_or_else(|| invalid_cursor("cursor payload ended unexpectedly"))?;
    *index += 1;
    Ok(value)
}

fn take_bytes<'a>(
    bytes: &'a [u8],
    index: &mut usize,
    length: usize,
) -> Result<&'a [u8], ToolError> {
    let end = index
        .checked_add(length)
        .ok_or_else(|| invalid_cursor("cursor field length overflowed"))?;
    let value = bytes
        .get(*index..end)
        .ok_or_else(|| invalid_cursor("cursor payload ended unexpectedly"))?;
    *index = end;
    Ok(value)
}

fn take_string(bytes: &[u8], index: &mut usize) -> Result<String, ToolError> {
    let length = take_byte(bytes, index)? as usize;
    let value = take_bytes(bytes, index, length)?;
    String::from_utf8(value.to_vec()).map_err(|_| invalid_cursor("cursor text is not valid UTF-8"))
}

fn decode_hex_fixed<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 {
        return None;
    }
    let mut output = [0; N];
    for (index, slot) in output.iter_mut().enumerate() {
        let start = index * 2;
        *slot = u8::from_str_radix(&value[start..start + 2], 16).ok()?;
    }
    Some(output)
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn invalid_cursor(message: impl Into<String>) -> ToolError {
    ToolError::new("invalid_cursor", message)
        .with_hint("omit cursor to restart the query from the first page")
}

fn resolve_source<'a>(
    workspace: &'a WorkspaceSnapshot,
    selector: &Selector,
) -> Result<ResolvedSource<'a>, ToolError> {
    let (module, label) = match selector {
        Selector::Module { module, label } => {
            let path = parse_absolute_module_path(module).ok_or_else(|| {
                ToolError::new(
                    "invalid_selector",
                    "module selector must be an absolute ModulePath",
                )
            })?;
            let module = workspace.module(&path).ok_or_else(|| {
                ToolError::new("not_found", format!("module `{module}` was not found"))
            })?;
            (module, label.clone())
        }
        Selector::Path { path, label } => {
            let absolute = if path.is_absolute() {
                path.clone()
            } else {
                workspace.root().join(path)
            };
            let absolute = dunce::canonicalize(&absolute).map_err(|_| {
                ToolError::new(
                    "not_found",
                    format!("source `{}` was not found", path.display()),
                )
            })?;
            if !absolute.starts_with(workspace.root()) {
                return Err(ToolError::new(
                    "invalid_selector",
                    "source path escapes the Vault",
                ));
            }
            let module = workspace.module_for_source(&absolute).ok_or_else(|| {
                ToolError::new(
                    "not_found",
                    format!("source `{}` is not a Notist module", path.display()),
                )
            })?;
            (module, label.clone())
        }
    };
    let file_id = module.file_id.ok_or_else(|| {
        ToolError::new(
            "not_found",
            "selector resolves to a virtual module without source",
        )
    })?;
    let source = workspace.source(file_id).unwrap();
    let selection = if let Some(label_name) = &label {
        match workspace.resolve_module_label(&module.logical_path, label_name) {
            RefTarget::Scope { .. } => workspace
                .label_scope_range(&module.logical_path, label_name)
                .unwrap_or(TextRange::new(0, source.text.len())),
            RefTarget::Missing(MissingReason::Ambiguous) => {
                return Err(ToolError::new(
                    "ambiguous_selector",
                    format!(
                        "label `{label_name}` in `{}` matches multiple headings; add an explicit `@id` to disambiguate",
                        module.logical_path
                    ),
                )
                .with_hint("use an explicit id or a more specific selector"));
            }
            RefTarget::Missing(_) => {
                return Err(ToolError::new(
                    "not_found",
                    format!(
                        "label `{label_name}` was not found in {}",
                        module.logical_path
                    ),
                ));
            }
            _ => {
                return Err(ToolError::new(
                    "not_found",
                    format!(
                        "label `{label_name}` in `{}` is not a section or scope target",
                        module.logical_path
                    ),
                ));
            }
        }
    } else {
        TextRange::new(0, source.text.len())
    };
    Ok(ResolvedSource {
        module,
        source,
        label,
        selection,
    })
}

fn location(
    workspace: &WorkspaceSnapshot,
    module: &notist_analysis::Module,
    source: &notist_analysis::SourceInput,
    range: TextRange,
    id: Option<String>,
) -> Location {
    Location {
        module: module.logical_path.to_string(),
        relative_path: relative_path(workspace.root(), &source.canonical_path),
        byte_range: range.into(),
        line_range: Some(line_range(&source.text, range)),
        id,
        source_fingerprint: fingerprint(&source.text),
    }
}

fn line_range(source: &str, range: TextRange) -> LineRange {
    let starts = line_starts(source);
    LineRange {
        start: starts.partition_point(|start| *start <= range.start).max(1),
        end: starts
            .partition_point(|start| *start < range.end.max(range.start + 1))
            .max(1),
    }
}

fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        source
            .bytes()
            .enumerate()
            .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
    );
    starts
}

fn excerpt(source: &str, range: TextRange, max_bytes: usize) -> (String, TextRange, bool) {
    if source.is_empty() {
        return (String::new(), TextRange::new(0, 0), false);
    }
    let match_len = range.end.saturating_sub(range.start);
    if match_len > max_bytes {
        let center = range.start + match_len / 2;
        let mut start = floor_char_boundary(source, center.saturating_sub(max_bytes / 2));
        let end = floor_char_boundary(source, (start + max_bytes).min(source.len()));
        start = floor_char_boundary(source, end.saturating_sub(max_bytes));
        return (
            source[start..end].to_owned(),
            TextRange::new(start, end),
            start > 0 || end < source.len(),
        );
    }
    let before = max_bytes.saturating_sub(match_len.min(max_bytes)) / 2;
    let mut start = floor_char_boundary(source, range.start.saturating_sub(before));
    let mut end = floor_char_boundary(source, (start + max_bytes).min(source.len()));
    if end < range.end.min(source.len()) {
        end = floor_char_boundary(source, range.end.min(source.len()));
        start = floor_char_boundary(source, end.saturating_sub(max_bytes));
    }
    let truncated = start > 0 || end < source.len();
    (
        source[start..end].to_owned(),
        TextRange::new(start, end),
        truncated,
    )
}

fn lexical_match_range(source: &str, unit: TextRange, query: &str) -> Option<TextRange> {
    if unit.start >= unit.end || unit.end > source.len() {
        return None;
    }
    let region = &source[unit.start..unit.end];
    for mut variants in query_term_groups(query) {
        variants.sort_by_key(|variant| std::cmp::Reverse(variant.chars().count()));
        for variant in variants {
            let pattern = RegexBuilder::new(&regex::escape(&variant))
                .case_insensitive(true)
                .build()
                .ok()?;
            if let Some(found) = pattern.find(region) {
                return Some(TextRange::new(
                    unit.start + found.start(),
                    unit.start + found.end(),
                ));
            }
        }
    }
    None
}

fn add_empty_search_hint(page: &mut QueryPage<SearchHit>, query: &SearchQuery) {
    if !page.items.is_empty() || !page.coverage.complete {
        return;
    }
    let hint = match query.mode {
        SearchMode::Lexical | SearchMode::Fuzzy if query.operator == SearchOperator::All => {
            "no matches; try fewer or simpler keywords, or set operator=any for broader recall"
        }
        SearchMode::Lexical | SearchMode::Fuzzy => {
            "no matches; try fewer or simpler keywords, or use exact mode for a known literal phrase"
        }
        SearchMode::Exact => {
            "no matches; try a shorter literal substring or lexical mode for token-based search"
        }
        SearchMode::Regex => {
            "no matches; simplify the pattern or use exact mode for a literal substring"
        }
    };
    page.hints.push(hint.into());
}

fn floor_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn semantic_units(source: &str) -> Vec<TextRange> {
    let mut units = Vec::new();
    let mut start = 0usize;
    for (index, _) in source.match_indices("\n\n") {
        let end = index + 1;
        split_unit(source, start, end, &mut units);
        start = index + 2;
    }
    split_unit(source, start, source.len(), &mut units);
    if units.is_empty() {
        units.push(TextRange::new(0, 0));
    }
    units
}

fn comment_ranges(source: &str) -> Vec<TextRange> {
    let bytes = source.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        index = (index + 2).min(bytes.len());
                    } else if bytes[index] == b'"' {
                        index += 1;
                        break;
                    } else {
                        index += 1;
                    }
                }
            }
            b'`' => {
                let start = index;
                while index < bytes.len() && bytes[index] == b'`' {
                    index += 1;
                }
                let width = index - start;
                while index < bytes.len() {
                    if bytes[index] == b'`' {
                        let close = index;
                        while index < bytes.len() && bytes[index] == b'`' {
                            index += 1;
                        }
                        if index - close >= width {
                            break;
                        }
                    } else {
                        index += 1;
                    }
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'/')
                && index.checked_sub(1).and_then(|before| bytes.get(before)) != Some(&b':') =>
            {
                let start = index;
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
                ranges.push(TextRange::new(start, index));
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                let start = index;
                index += 2;
                let mut depth = 1usize;
                while index < bytes.len() && depth > 0 {
                    if bytes[index..].starts_with(b"/*") {
                        depth += 1;
                        index += 2;
                    } else if bytes[index..].starts_with(b"*/") {
                        depth -= 1;
                        index += 2;
                    } else {
                        index += 1;
                    }
                }
                ranges.push(TextRange::new(start, index));
            }
            _ => index += 1,
        }
    }
    ranges
}

fn merge_ranges(mut ranges: Vec<TextRange>) -> Vec<TextRange> {
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<TextRange> = Vec::new();
    for range in ranges {
        if let Some(previous) = merged.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

fn complement_ranges(length: usize, excluded: &[TextRange]) -> Vec<TextRange> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for excluded in excluded {
        if start < excluded.start {
            ranges.push(TextRange::new(start, excluded.start.min(length)));
        }
        start = start.max(excluded.end.min(length));
    }
    if start < length {
        ranges.push(TextRange::new(start, length));
    }
    ranges
}

fn text_excluding(source: &str, range: TextRange, excluded: &[TextRange]) -> String {
    let mut output = String::new();
    let mut start = range.start;
    for excluded in excluded {
        if excluded.end <= range.start || excluded.start >= range.end {
            continue;
        }
        let excluded_start = excluded.start.max(range.start);
        if start < excluded_start {
            output.push_str(&source[start..excluded_start]);
            output.push(' ');
        }
        start = start.max(excluded.end.min(range.end));
    }
    if start < range.end {
        output.push_str(&source[start..range.end]);
    }
    output
}

fn split_unit(source: &str, mut start: usize, end: usize, output: &mut Vec<TextRange>) {
    while start < end {
        let chunk_end = floor_char_boundary(source, (start + 4096).min(end));
        output.push(TextRange::new(start, chunk_end));
        if chunk_end >= end {
            break;
        }
        start = floor_char_boundary(source, chunk_end.saturating_sub(256)).max(start + 1);
    }
}

fn normalize_for_index(value: &str) -> String {
    query_terms(value).join(" ")
}

fn query_terms(value: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    query_term_groups(value)
        .into_iter()
        .flatten()
        .filter(|term| seen.insert(term.clone()))
        .collect()
}

fn query_term_groups(value: &str) -> Vec<Vec<String>> {
    let normalized = value.nfkc().collect::<String>();
    let mut groups = Vec::new();
    let mut word = String::new();
    let mut han = String::new();
    let flush_word = |word: &mut String, groups: &mut Vec<Vec<String>>| {
        if word.is_empty() {
            return;
        }
        let mut variants = vec![word.to_lowercase()];
        let mut part = String::new();
        let chars = word.chars().collect::<Vec<_>>();
        for (index, character) in chars.iter().enumerate() {
            if index > 0 && character.is_uppercase() && chars[index - 1].is_lowercase() {
                if !part.is_empty() {
                    variants.push(part.to_lowercase());
                    part.clear();
                }
            }
            part.push(*character);
        }
        if !part.is_empty() && part != *word {
            variants.push(part.to_lowercase());
        }
        variants.sort();
        variants.dedup();
        groups.push(variants);
        word.clear();
    };
    let flush_han = |han: &mut String, groups: &mut Vec<Vec<String>>| {
        let chars = han.chars().collect::<Vec<_>>();
        if chars.len() == 1 {
            groups.push(vec![chars[0].to_string()]);
        } else {
            for pair in chars.windows(2) {
                groups.push(vec![pair.iter().collect()]);
            }
        }
        han.clear();
    };
    for character in normalized.chars() {
        if is_han(character) {
            flush_word(&mut word, &mut groups);
            han.push(character);
        } else if character.is_alphanumeric() {
            flush_han(&mut han, &mut groups);
            word.push(character);
        } else {
            flush_word(&mut word, &mut groups);
            flush_han(&mut han, &mut groups);
        }
    }
    flush_word(&mut word, &mut groups);
    flush_han(&mut han, &mut groups);
    groups
}

fn is_han(character: char) -> bool {
    matches!(character as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

fn fuzzy_eligible(term: &str) -> bool {
    term.chars().count() >= 4
        && term.chars().any(|character| character.is_alphabetic())
        && !term.chars().any(is_han)
}

fn in_scope(module: &str, scopes: &[String]) -> bool {
    scopes.is_empty()
        || scopes.iter().any(|scope| {
            module == scope
                || module
                    .strip_prefix(scope)
                    .is_some_and(|suffix| suffix.starts_with("::"))
        })
}

fn search_fingerprint(query: &SearchQuery) -> String {
    serde_json::to_string(&(
        &query.query,
        query.mode,
        &query.scopes,
        &query.fields,
        query.operator,
        query.applied_group_by(),
        query.ignore_case,
        query.fuzzy_distance,
        query.wait_index_ms,
        RANKING_VERSION,
        TOKENIZER_VERSION,
        INDEX_SCHEMA_VERSION,
    ))
    .unwrap()
}

fn relative_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn fingerprint(source: &str) -> String {
    digest(source.as_bytes())[..16].to_owned()
}

fn digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn parse_absolute_module_path(value: &str) -> Option<ModulePath> {
    if value == "vault" {
        return Some(ModulePath::root());
    }
    let tail = value.strip_prefix("vault::")?;
    let segments = tail
        .split("::")
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    (!segments.is_empty()).then(|| ModulePath::from_segments(segments))
}

fn diagnostic_code(kind: &DiagnosticKind) -> &'static str {
    match kind {
        DiagnosticKind::DuplicateModule => "duplicate-module",
        DiagnosticKind::DuplicateLabel => "duplicate-label",
        DiagnosticKind::InvalidSyntax => "invalid-syntax",
        DiagnosticKind::UnresolvedModule => "unresolved-module",
        DiagnosticKind::UnresolvedLabel => "unresolved-label",
        DiagnosticKind::AmbiguousLabel => "ambiguous-label",
        DiagnosticKind::UnknownFunction => "unknown-function",
        DiagnosticKind::DuplicateFunction => "duplicate-function",
        DiagnosticKind::UnresolvedName => "unresolved-name",
        DiagnosticKind::InvalidFunction => "invalid-function",
        DiagnosticKind::InvalidArguments => "invalid-arguments",
        DiagnosticKind::TypeMismatch => "type-mismatch",
        DiagnosticKind::Evaluation => "evaluation",
        DiagnosticKind::ExternalReferenceUnsupported => "external-reference-unsupported",
        DiagnosticKind::ImportCycle => "import-cycle",
    }
}

pub fn status(
    workspace: &WorkspaceSnapshot,
    snapshot: &SnapshotIdentity,
    kind: ViewKind,
    runtime_mode: &str,
    index: IndexStatusRecord,
) -> StatusRecord {
    StatusRecord {
        root: workspace.root().to_path_buf(),
        source_count: workspace.sources().count(),
        module_count: workspace.modules().count(),
        diagnostic_count: workspace.diagnostics().len(),
        runtime_mode: runtime_mode.into(),
        view_kind: match kind {
            ViewKind::Disk => "disk",
            ViewKind::Session => "session",
        }
        .into(),
        snapshot: snapshot.clone(),
        index,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DaemonInstanceId, ServiceViewId, VaultIdentity};

    #[test]
    fn tokenizer_preserves_identifiers_and_han_bigrams() {
        let terms = query_terms("WorkspaceSnapshot 研究生毕设");
        assert!(terms.contains(&"workspacesnapshot".into()));
        assert!(terms.contains(&"workspace".into()));
        assert!(terms.contains(&"snapshot".into()));
        assert!(terms.contains(&"研究".into()));
        assert!(terms.contains(&"毕设".into()));
        let groups = query_term_groups("WorkspaceSnapshot");
        assert_eq!(groups.len(), 1);
        assert!(groups[0].contains(&"workspace".into()));
    }

    #[test]
    fn cursor_detects_query_changes() {
        let cursor = encode_cursor("search", &snapshot("source"), "query-a", 12);
        assert!(!cursor.contains('.'));
        assert!(cursor.len() < 128);
        let decoded = decode_cursor(&cursor).unwrap();
        assert_eq!(decoded.offset, 12);
        assert_eq!(decoded.query_fingerprint, digest(b"query-a"));

        let legacy = encode_legacy_cursor("search", &snapshot("source"), "query-a", 12);
        assert_eq!(decode_cursor(&legacy).unwrap().offset, 12);
    }

    #[test]
    fn search_grouping_defaults_follow_discovery_semantics() {
        let mut query = SearchQuery {
            query: "needle".into(),
            mode: SearchMode::Lexical,
            scopes: Vec::new(),
            fields: SearchField::defaults(),
            operator: SearchOperator::All,
            group_by: None,
            ignore_case: false,
            fuzzy_distance: 1,
            wait_index_ms: 2000,
            snippet_bytes: DEFAULT_SNIPPET_BYTES,
            page: PageRequest::default(),
        };
        assert_eq!(query.applied_group_by(), SearchGroup::Source);
        query.mode = SearchMode::Exact;
        assert_eq!(query.applied_group_by(), SearchGroup::Match);
        query.group_by = Some(SearchGroup::Section);
        assert_eq!(query.applied_group_by(), SearchGroup::Section);
    }

    fn snapshot(source: &str) -> SnapshotIdentity {
        SnapshotIdentity {
            daemon_instance: DaemonInstanceId("test".into()),
            vault: VaultIdentity {
                canonical_root: PathBuf::from("vault"),
                fingerprint: "0123456789abcdef".into(),
            },
            view_id: ServiceViewId(1),
            view_kind: "disk".into(),
            analyzer_view_id: 1,
            revision: 1,
            source_fingerprint: fingerprint(source),
        }
    }

    #[test]
    fn page_honors_byte_budget_and_stale_cursor() {
        let first = page(
            &snapshot("one"),
            "test",
            "query",
            &PageRequest {
                limit: Some(100),
                max_bytes: Some(4096),
                cursor: None,
            },
            20,
            vec!["x".repeat(1800), "y".repeat(1800), "z".repeat(1800)],
        )
        .unwrap();
        assert!(serde_json::to_vec(&first).unwrap().len() <= 4096);
        assert!(first.page.has_more);
        assert!(first.hints[0].contains("repeat the same query"));
        let error = page(
            &snapshot("two"),
            "test",
            "query",
            &PageRequest {
                limit: Some(1),
                max_bytes: Some(4096),
                cursor: first.page.next_cursor,
            },
            20,
            vec![String::from("x")],
        )
        .unwrap_err();
        assert_eq!(error.code, "cursor_stale");

        let first = page(
            &snapshot("one"),
            "test",
            "query-a",
            &PageRequest {
                limit: Some(1),
                max_bytes: Some(4096),
                cursor: None,
            },
            20,
            vec![String::from("x"), String::from("y")],
        )
        .unwrap();
        let error = page(
            &snapshot("one"),
            "test",
            "query-b",
            &PageRequest {
                limit: Some(1),
                max_bytes: Some(4096),
                cursor: first.page.next_cursor,
            },
            20,
            vec![String::from("x"), String::from("y")],
        )
        .unwrap_err();
        assert_eq!(error.code, "invalid_cursor");
        assert!(error.hint.unwrap().contains("original selector"));
    }

    #[test]
    fn excerpts_never_exceed_requested_bytes() {
        let source = "a".repeat(10_000);
        let (excerpt, _, truncated) = excerpt(&source, TextRange::new(5000, 5010), 512);
        assert!(excerpt.len() <= 512);
        assert!(truncated);
    }

    #[test]
    fn lexical_excerpt_is_anchored_to_a_query_term() {
        let source = format!("{}ImportantNeedle{}", "a".repeat(700), "z".repeat(700));
        let unit = TextRange::new(0, source.len());
        let matched = lexical_match_range(&source, unit, "important needle").unwrap();
        assert_eq!(&source[matched.start..matched.end], "Important");
        let (excerpt, excerpt_range, _) = excerpt(&source, matched, 128);
        assert!(excerpt.contains("Important"));
        assert!(excerpt_range.start <= matched.start && matched.end <= excerpt_range.end);
    }

    #[test]
    fn empty_search_page_carries_actionable_hint() {
        let mut result = page::<SearchHit>(
            &snapshot("one"),
            "search",
            "query",
            &PageRequest::default(),
            SEARCH_DEFAULT_LIMIT,
            Vec::new(),
        )
        .unwrap();
        add_empty_search_hint(
            &mut result,
            &SearchQuery {
                query: "too many words".into(),
                mode: SearchMode::Lexical,
                scopes: Vec::new(),
                fields: SearchField::defaults(),
                operator: SearchOperator::All,
                group_by: None,
                ignore_case: false,
                fuzzy_distance: 1,
                wait_index_ms: 2000,
                snippet_bytes: 512,
                page: PageRequest::default(),
            },
        );
        assert!(result.hints[0].contains("operator=any"));
    }

    #[test]
    fn comment_scanner_keeps_urls_and_skips_raw() {
        let source = "https://example.com\n// searchable comment\n`// raw`\n/* nested /* block */ comment */";
        let ranges = comment_ranges(source);
        let comments = ranges
            .iter()
            .map(|range| &source[range.start..range.end])
            .collect::<Vec<_>>();
        assert_eq!(comments.len(), 2);
        assert!(comments[0].contains("searchable comment"));
        assert!(comments[1].contains("nested"));
    }
}
