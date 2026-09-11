//! Dense (vector) retrieval over a Vault: structural chunks, an embedder
//! abstraction, a rebuildable local vector file, and brute-force cosine
//! queries. This is the `notist vsearch` experiment lane — it shares no
//! contract with the lexical search family and touches nothing there.
//!
//! Embeddings come exclusively from a user-provided OpenAI-compatible
//! endpoint (ollama, llama-server, cloud APIs): the binary embeds nothing
//! itself and carries no inference engine — not even a model download
//! path. Provisioning the server is the user's setup.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notist_analysis::WorkspaceSnapshot;
use notist_model::TextRange;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use crate::query::{LineRange, Location, QueryResult, ToolError};
use crate::request::ByteRange;
use crate::{SnapshotIdentity, digest};

/// Version of the chunking scheme. Bump to invalidate every stored vector.
pub const CHUNKER_VERSION: &str = "vector-chunks-v1";

/// How one document is cut into embeddable blocks. Both derive the block
/// boundaries from the evaluated Item tree; the granularity only decides how
/// fine the leaves are.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkGranularity {
    /// One block per heading section (the section's ItemId range).
    #[default]
    Section,
    /// Heading sections split further at blank lines; every block carries its
    /// heading chain as context.
    Paragraph,
    /// Heading sections partitioned at their direct `#[...]` scope children:
    /// a scope block becomes its own unit, the section text around the scopes
    /// forms head/tail remainder units. Falls back to plain sections where a
    /// document declares no scopes.
    Scope,
}

impl ChunkGranularity {
    fn parse(value: &str) -> io::Result<Self> {
        match value {
            "section" => Ok(Self::Section),
            "paragraph" => Ok(Self::Paragraph),
            "scope" => Ok(Self::Scope),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "unknown embedding granularity `{other}` (expected `section`, `paragraph`, or `scope`)"
                ),
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Section => "section",
            Self::Paragraph => "paragraph",
            Self::Scope => "scope",
        }
    }
}

/// Extra context prepended to every block's embedding text. The body of a
/// section chunk already contains its own heading line; the chain adds the
/// full ancestor path, and attributes fold the block's effective annotation
/// environment (module `@!` plus the governing section's annotations) into
/// the text.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkContext {
    /// Body text only.
    #[default]
    None,
    /// `heading/chain` line, then body.
    Chain,
    /// `heading/chain` line, then `key = value` attribute tokens, then body.
    ChainAttrs,
}

impl ChunkContext {
    fn parse(value: &str) -> io::Result<Self> {
        match value {
            "none" => Ok(Self::None),
            "chain" => Ok(Self::Chain),
            "chain-attrs" => Ok(Self::ChainAttrs),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "unknown embedding context `{other}` (expected `none`, `chain`, or `chain-attrs`)"
                ),
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Chain => "chain",
            Self::ChainAttrs => "chain-attrs",
        }
    }
}

/// `[embedding]` table of the Vault `Notist.toml`. Absence of the table
/// simply disables the dense lane.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddingConfig {
    /// Model identifier forwarded to the endpoint verbatim.
    pub model: String,
    /// OpenAI-compatible `/v1` base URL (ollama, llama-server, cloud APIs).
    pub endpoint: String,
    /// Expected vector width. Optional: when unset, the width is learned
    /// from the first embedding response and verified against the stored
    /// index.
    pub dims: Option<usize>,
    /// Environment variable carrying the endpoint bearer token.
    pub api_key_env: Option<String>,
    pub granularity: ChunkGranularity,
    pub context: ChunkContext,
}

impl EmbeddingConfig {
    /// Reads `[embedding]` from the Vault manifest. A missing table or a
    /// missing manifest yields `None`; a malformed table is an error.
    pub fn from_vault_root(root: &Path) -> io::Result<Option<Self>> {
        let path = root.join("Notist.toml");
        if !path.is_file() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)?;
        // A manifest that fails TOML parsing belongs to the plugin host's
        // diagnostics, not to this lane; we simply stay disabled.
        let Ok(value) = toml::from_str::<toml::Value>(&text) else {
            return Ok(None);
        };
        let Some(table) = value.get("embedding").and_then(|value| value.as_table()) else {
            return Ok(None);
        };
        let model = table
            .get("model")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "[embedding] model must be a string",
                )
            })?
            .to_string();
        let endpoint = table
            .get("endpoint")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "[embedding] endpoint must be a string — the lane ships no built-in                      inference; point it at an OpenAI-compatible /v1 server, e.g.                      \"http://127.0.0.1:11434/v1\" for ollama",
                )
            })?
            .to_string();
        let dims = table
            .get("dims")
            .and_then(|value| value.as_integer())
            .map(|value| value as usize);
        let api_key_env = table
            .get("api_key_env")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let granularity = match table.get("granularity").and_then(|value| value.as_str()) {
            None => ChunkGranularity::default(),
            Some(value) => ChunkGranularity::parse(value)?,
        };
        let context = match table.get("context").and_then(|value| value.as_str()) {
            None => ChunkContext::default(),
            Some(value) => ChunkContext::parse(value)?,
        };
        let config = Self {
            model,
            endpoint,
            dims,
            api_key_env,
            granularity,
            context,
        };
        config.validate()?;
        Ok(Some(config))
    }

    fn validate(&self) -> io::Result<()> {
        if self.dims == Some(0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "[embedding] dims must be a positive integer",
            ));
        }
        Ok(())
    }

    /// Stable identity of the embedding function; any change invalidates the
    /// stored vectors (different models produce incomparable spaces).
    pub fn model_id(&self) -> String {
        format!(
            "endpoint:{}#{}",
            self.endpoint.trim_end_matches('/'),
            self.model
        )
    }
}

/// The embedding function abstraction. The vector width is learned from the
/// first response rather than declared up front: provisioning belongs to
/// the server, and `dims` in the config is a check, not a requirement.
pub trait Embedder {
    fn model_id(&self) -> String;
    /// Embeds a batch of texts, returning one vector per input in order.
    fn embed(&mut self, texts: &[String]) -> io::Result<Vec<Vec<f32>>>;
}

// ---------------------------------------------------------------- endpoint

/// OpenAI-compatible `/v1/embeddings` client (ollama, llama-server, cloud
/// APIs all speak this shape).
pub struct EndpointEmbedder {
    agent: ureq::Agent,
    url: String,
    model: String,
    auth: Option<String>,
    id: String,
}

const ENDPOINT_BATCH: usize = 32;

/// Endpoint inputs are clipped to this many characters: built-in models
/// truncate silently at their context window (bge-small-zh: 512 tokens) and
/// several endpoint servers reject over-long inputs outright. 400 chars is
/// ≈ under 512 tokens for both CJK-dense and ASCII text.
const ENDPOINT_MAX_INPUT_CHARS: usize = 400;

impl EndpointEmbedder {
    pub fn new(config: &EmbeddingConfig) -> io::Result<Self> {
        let auth = match &config.api_key_env {
            None => None,
            Some(var) => match std::env::var(var) {
                Ok(key) if !key.is_empty() => Some(format!("Bearer {key}")),
                Ok(_) => None,
                Err(_) => {
                    return Err(io::Error::other(format!(
                        "environment variable {var} ([embedding].api_key_env) is not set"
                    )));
                }
            },
        };
        Ok(Self {
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(120))
                .build(),
            url: format!("{}/embeddings", config.endpoint.trim_end_matches('/')),
            model: config.model.clone(),
            auth,
            id: config.model_id(),
        })
    }
}

impl Embedder for EndpointEmbedder {
    fn model_id(&self) -> String {
        self.id.clone()
    }

    fn embed(&mut self, texts: &[String]) -> io::Result<Vec<Vec<f32>>> {
        let clipped: Vec<String> = texts
            .iter()
            .map(|text| text.chars().take(ENDPOINT_MAX_INPUT_CHARS).collect())
            .collect();
        let mut vectors = Vec::with_capacity(clipped.len());
        for batch in clipped.chunks(ENDPOINT_BATCH) {
            let mut request = self
                .agent
                .post(&self.url)
                .set("Content-Type", "application/json");
            if let Some(auth) = &self.auth {
                request = request.set("Authorization", auth);
            }
            let response = request
                .send_json(serde_json::json!({ "model": self.model, "input": batch }))
                .map_err(|error| io::Error::other(format!("embedding endpoint failed: {error}")))?;
            let payload: serde_json::Value = response
                .into_json()
                .map_err(|error| io::Error::other(format!("embedding endpoint reply: {error}")))?;
            let mut data: Vec<(usize, Vec<f32>)> = payload
                .get("data")
                .and_then(|data| data.as_array())
                .ok_or_else(|| io::Error::other("embedding endpoint reply has no `data` array"))?
                .iter()
                .enumerate()
                .map(|(fallback, item)| {
                    let index = item
                        .get("index")
                        .and_then(|index| index.as_u64())
                        .map(|index| index as usize)
                        .unwrap_or(fallback);
                    let vector = item
                        .get("embedding")
                        .and_then(|embedding| embedding.as_array())
                        .ok_or_else(|| {
                            io::Error::other("embedding endpoint reply item has no `embedding`")
                        })?
                        .iter()
                        .map(|value| {
                            value.as_f64().map(|value| value as f32).ok_or_else(|| {
                                io::Error::other("embedding vector entry is not a number")
                            })
                        })
                        .collect::<io::Result<Vec<f32>>>()?;
                    Ok((index, vector))
                })
                .collect::<io::Result<Vec<(usize, Vec<f32>)>>>()?;
            data.sort_by_key(|(index, _)| *index);
            if data.len() != batch.len() {
                return Err(io::Error::other(format!(
                    "embedding endpoint returned {} vectors for {} inputs",
                    data.len(),
                    batch.len()
                )));
            }
            vectors.extend(data.into_iter().map(|(_, vector)| vector));
        }
        Ok(vectors)
    }
}

// ------------------------------------------------------------------ chunks

/// A content block and its address. Persisted; the block text itself is
/// recoverable from the source via `byte_range`, so it is not stored.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VectorChunk {
    pub module: String,
    pub relative_path: PathBuf,
    pub heading_path: String,
    pub byte_range: (usize, usize),
    pub line_range: (usize, usize),
    pub excerpt: String,
    pub hash: String,
}

struct DraftChunk {
    chunk: VectorChunk,
    text: String,
}

fn content_hash(
    model_id: &str,
    granularity: ChunkGranularity,
    context: ChunkContext,
    heading: &str,
    text: &str,
) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(CHUNKER_VERSION.as_bytes());
    hasher.update([0]);
    hasher.update(model_id.as_bytes());
    hasher.update([0]);
    hasher.update(granularity.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(context.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(heading.as_bytes());
    hasher.update([0]);
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..24]
        .to_string()
}

/// 1-based inclusive line range covering a byte range of `text`.
fn line_range_of(text: &str, range: (usize, usize)) -> (usize, usize) {
    let line_at = |offset: usize| {
        text.as_bytes()[..offset]
            .iter()
            .filter(|b| **b == b'\n')
            .count()
            + 1
    };
    let start = line_at(range.0);
    let end_anchor = range.1.max(range.0 + 1) - 1;
    (start, line_at(end_anchor))
}

fn excerpt_of(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let mut excerpt: String = line.chars().take(96).collect();
    if line.chars().count() > 96 {
        excerpt.push('…');
    }
    excerpt
}

/// Cuts one section into blank-line-delimited paragraph drafts. Blank lines
/// and wholly-comment lines break paragraphs; each draft's text carries the
/// heading chain so the block stays self-describing after the cut.
#[allow(clippy::too_many_arguments)]
fn paragraph_drafts(
    model_id: &str,
    module: &str,
    relative: &Path,
    chain: &str,
    section: TextRange,
    source: &str,
    comments: &[TextRange],
) -> Vec<DraftChunk> {
    let mut paragraphs: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut group: Option<(usize, usize, Vec<String>)> = None;
    let mut offset = section.start;
    for line in source[section.start..section.end].split_inclusive('\n') {
        let line_start = offset;
        let line_end = offset + line.trim_end_matches(['\n', '\r']).len();
        offset += line.len();
        // Heading lines stay out of paragraph blocks: the heading chain
        // prefix already carries that context.
        let is_heading = line.starts_with('=');
        let is_blank = line.trim().is_empty();
        let is_comment = comments
            .iter()
            .any(|comment| comment.start <= line_start && line_end <= comment.end);
        if is_heading || is_blank || is_comment {
            if let Some(spans) = group.take() {
                paragraphs.push(spans);
            }
        } else {
            let text = line.trim_end_matches(['\n', '\r']);
            match &mut group {
                Some((_, end, lines)) => {
                    *end = line_end;
                    lines.push(text.to_string());
                }
                None => group = Some((line_start, line_end, vec![text.to_string()])),
            }
        }
    }
    if let Some(spans) = group.take() {
        paragraphs.push(spans);
    }
    paragraphs
        .into_iter()
        .map(|(start, end, lines)| {
            let body = lines.join("\n");
            let text = format!("{chain}\n{body}");
            make_draft(
                model_id,
                ChunkGranularity::Paragraph,
                ChunkContext::None,
                module,
                relative,
                chain,
                (start, end),
                source,
                body,
                text,
            )
        })
        .collect()
}

/// One embeddable heading section with its evaluated node, so scope-aware
/// granularities can partition it.
struct SectionUnit<'a> {
    chain: String,
    range: TextRange,
    node: &'a notist_model::Node,
}

fn collect_section_units<'a>(
    nodes: &'a [notist_model::Node],
    chain: &mut Vec<String>,
    out: &mut Vec<SectionUnit<'a>>,
) {
    for node in nodes {
        if node.is_core("section") {
            let title = node
                .children
                .first()
                .filter(|child| child.is_core("heading"))
                .map(|heading| notist_analysis::node_text(&heading.children))
                .unwrap_or_default();
            chain.push(title);
            out.push(SectionUnit {
                chain: chain.join("/"),
                range: node.range,
                node,
            });
            collect_section_units(&node.children, chain, out);
            for (_, value) in &node.args {
                if let notist_model::NodeValue::Stream(stream) = value {
                    collect_section_units(stream, chain, out);
                }
            }
            chain.pop();
            continue;
        }
        collect_section_units(&node.children, chain, out);
        for (_, value) in &node.args {
            if let notist_model::NodeValue::Stream(stream) = value {
                collect_section_units(stream, chain, out);
            }
        }
    }
}

/// Partitions a section at its direct `#[...]` scope children: heading head,
/// each scope block, the gaps between them, and the tail. Sections without
/// scope children degrade to a single whole-section unit.
fn scope_partition(unit: &SectionUnit) -> Vec<(TextRange, String)> {
    let scopes: Vec<&notist_model::Node> = unit
        .node
        .children
        .iter()
        .skip(1) // the heading
        .filter(|child| child.block && child.name == "scope")
        .collect();
    if scopes.is_empty() {
        return vec![(unit.range, unit.chain.clone())];
    }
    let mut units = Vec::new();
    let mut cursor = unit.range.start;
    for scope in scopes {
        if scope.range.start > cursor {
            units.push((
                TextRange::new(cursor, scope.range.start),
                unit.chain.clone(),
            ));
        }
        units.push((scope.range, unit.chain.clone()));
        cursor = cursor.max(scope.range.end);
    }
    if cursor < unit.range.end {
        units.push((TextRange::new(cursor, unit.range.end), unit.chain.clone()));
    }
    units
}

/// Effective annotation environment of one section: the module's `@![...]`
/// attributes plus annotation entries governing the section. An entry
/// governs the smallest section that starts at or after the entry's own
/// bytes — a pre-heading `@( ... )` therefore lands on the section it binds,
/// and mid-section entries land on the subsection they precede.
fn section_attributes(
    workspace: &WorkspaceSnapshot,
    module_id: notist_analysis::ModuleId,
    units: &[SectionUnit],
) -> HashMap<usize, Vec<(String, String)>> {
    let mut by_unit: HashMap<usize, Vec<(String, String)>> = HashMap::new();
    for batch in workspace.module_attributes(module_id) {
        for (key, value) in batch {
            by_unit
                .entry(usize::MAX) // module attributes apply everywhere
                .or_default()
                .push((key.clone(), value.clone()));
        }
    }
    let structured = workspace.structured_module(module_id);
    let annotations = structured.as_ref().map(|s| s.annotations.as_slice());
    if let Some(annotations) = annotations {
        // Units are sorted by range.start; each entry binds the first unit
        // that starts at or after the annotation itself.
        for entry in annotations {
            match units
                .iter()
                .enumerate()
                .find(|(_, unit)| unit.range.start >= entry.range.start)
            {
                Some((index, _)) => {
                    for (key, value) in &entry.attributes {
                        by_unit
                            .entry(units[index].range.start)
                            .or_default()
                            .push((key.clone(), value.clone()));
                    }
                }
                None => continue,
            }
        }
    }
    by_unit
}

/// Serializes an effective attribute environment into compact embedding-text
/// tokens. Long free-text values (todo notes) are clipped: the token's job
/// is status signal, not content.
fn attribute_tokens(attributes: &[(String, String)]) -> String {
    let mut tokens: Vec<String> = Vec::new();
    let mut budget = 240usize;
    for (key, value) in attributes {
        if budget == 0 {
            break;
        }
        let value = value.trim();
        let mut token = if value.is_empty() {
            format!("@{key}")
        } else {
            let clipped: String = value.chars().take(60).collect();
            let ellipsis = if value.chars().count() > 60 {
                "…"
            } else {
                ""
            };
            format!("@{key} = {clipped}{ellipsis}")
        };
        if token.len() > budget {
            token = token.chars().take(budget).collect();
            budget = 0;
        } else {
            budget -= token.len();
        }
        tokens.push(token);
    }
    tokens.join(" ")
}

/// Assembles the embedding text for one block under the configured context:
/// heading chain line and attribute tokens precede the body, each on its own
/// line, only when selected.
fn embedding_text(
    context: ChunkContext,
    chain: &str,
    attributes: Option<&[(String, String)]>,
    body: &str,
) -> String {
    match context {
        ChunkContext::None => body.to_string(),
        ChunkContext::Chain => format!("{chain}\n{body}"),
        ChunkContext::ChainAttrs => {
            let tokens = attributes
                .filter(|attributes| !attributes.is_empty())
                .map(attribute_tokens)
                .unwrap_or_default();
            if tokens.is_empty() {
                format!("{chain}\n{body}")
            } else {
                format!("{chain}\n{tokens}\n{body}")
            }
        }
    }
}

/// Cuts every source module into embeddable blocks on the evaluated Item
/// tree. Section granularity takes whole heading sections; paragraph
/// granularity splits sections at blank lines; scope granularity partitions
/// sections at their declared `#[...]` scope children.
fn build_chunks(
    workspace: &WorkspaceSnapshot,
    config: &EmbeddingConfig,
) -> io::Result<Vec<DraftChunk>> {
    let model_id = config.model_id();
    let mut drafts = Vec::new();
    for module in workspace
        .modules()
        .filter(|module| module.file_id.is_some())
    {
        let file_id = module.file_id.unwrap();
        let Some(source) = workspace.source(file_id) else {
            continue;
        };
        let Some(structured) = workspace.structured_module(module.id) else {
            continue;
        };
        let mut units: Vec<SectionUnit> = Vec::new();
        let mut chain = Vec::new();
        collect_section_units(&structured.tree.roots, &mut chain, &mut units);
        units.sort_by_key(|unit| unit.range.start);
        let comments = crate::query::comment_ranges(&source.text);
        let relative = crate::query::relative_path(workspace.root(), &source.canonical_path);
        let module_name = module.logical_path.to_string();
        let attributes = section_attributes(workspace, module.id, &units);
        let module_wide = attributes.get(&usize::MAX).cloned().unwrap_or_default();

        let emit = |drafts: &mut Vec<DraftChunk>,
                    granularity: ChunkGranularity,
                    chain: &str,
                    unit_start: usize,
                    byte_range: (usize, usize),
                    body: String| {
            if body.trim().is_empty() {
                return;
            }
            let env = match config.context {
                ChunkContext::ChainAttrs => {
                    let mut owned = module_wide.clone();
                    if let Some(own) = attributes.get(&unit_start) {
                        owned.extend(own.iter().cloned());
                    }
                    Some(owned)
                }
                _ => None,
            };
            let text = embedding_text(config.context, chain, env.as_deref(), &body);
            drafts.push(make_draft(
                &model_id,
                granularity,
                config.context,
                &module_name,
                &relative,
                chain,
                byte_range,
                &source.text,
                body,
                text,
            ));
        };

        for unit in &units {
            match config.granularity {
                ChunkGranularity::Section | ChunkGranularity::Scope => {
                    // Scope granularity partitions sections at their declared
                    // `#[...]` scope children; without any, both cut the same
                    // whole-section units.
                    for (range, chain) in scope_partition(unit) {
                        let body = crate::query::text_excluding(&source.text, range, &comments);
                        emit(
                            &mut drafts,
                            config.granularity,
                            &chain,
                            unit.range.start,
                            (range.start, range.end),
                            body,
                        );
                    }
                }
                ChunkGranularity::Paragraph => {
                    drafts.extend(paragraph_drafts(
                        &model_id,
                        &module_name,
                        &relative,
                        &unit.chain,
                        unit.range,
                        &source.text,
                        &comments,
                    ));
                }
            }
        }
    }
    Ok(drafts)
}

#[allow(clippy::too_many_arguments)]
fn make_draft(
    model_id: &str,
    granularity: ChunkGranularity,
    context: ChunkContext,
    module: &str,
    relative: &Path,
    chain: &str,
    byte_range: (usize, usize),
    source: &str,
    body: String,
    text: String,
) -> DraftChunk {
    let line_range = line_range_of(source, byte_range);
    DraftChunk {
        chunk: VectorChunk {
            module: module.to_string(),
            relative_path: relative.to_path_buf(),
            heading_path: chain.to_string(),
            byte_range,
            line_range,
            excerpt: excerpt_of(&body),
            hash: content_hash(model_id, granularity, context, chain, &text),
        },
        text,
    }
}

// ------------------------------------------------------------------- store

/// Everything that invalidates the stored vectors lives here; any mismatch
/// with the live config or snapshot means the file is discarded and rebuilt
/// (embeddings for unchanged blocks survive via the reuse cache).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DenseManifest {
    pub model_id: String,
    pub dims: usize,
    pub chunker_version: String,
    pub granularity: ChunkGranularity,
    pub context: ChunkContext,
    pub source_fingerprint: String,
    pub count: usize,
}

impl DenseManifest {
    fn matches(&self, config: &EmbeddingConfig, identity: &SnapshotIdentity) -> bool {
        self.model_id == config.model_id()
            && self.chunker_version == CHUNKER_VERSION
            && self.granularity == config.granularity
            && self.context == config.context
            && self.source_fingerprint == identity.source_fingerprint
    }
}

/// The dense index of one snapshot: chunk addresses plus a flat row-major
/// vector matrix (`count` rows of `dims` f32s).
#[derive(Clone, Debug)]
pub struct DenseIndex {
    pub manifest: DenseManifest,
    pub chunks: Vec<VectorChunk>,
    vectors: Vec<f32>,
}

impl DenseIndex {
    fn load(dir: &Path) -> Option<Self> {
        let manifest: DenseManifest =
            serde_json::from_str(&fs::read_to_string(dir.join("manifest.json")).ok()?).ok()?;
        let chunks: Vec<VectorChunk> =
            postcard::from_bytes(&fs::read(dir.join("chunks.bin")).ok()?).ok()?;
        let raw = fs::read(dir.join("vectors.bin")).ok()?;
        if raw.len() != manifest.count * manifest.dims * 4 || chunks.len() != manifest.count {
            return None;
        }
        let vectors = raw
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        Some(Self {
            manifest,
            chunks,
            vectors,
        })
    }

    fn store(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        fs::write(
            dir.join("manifest.json.tmp"),
            serde_json::to_vec_pretty(&self.manifest)?,
        )?;
        fs::rename(dir.join("manifest.json.tmp"), dir.join("manifest.json"))?;
        fs::write(
            dir.join("chunks.bin.tmp"),
            postcard::to_allocvec(&self.chunks)
                .map_err(|error| io::Error::other(error.to_string()))?,
        )?;
        fs::rename(dir.join("chunks.bin.tmp"), dir.join("chunks.bin"))?;
        let raw: Vec<u8> = self
            .vectors
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        fs::write(dir.join("vectors.bin.tmp"), &raw)?;
        fs::rename(dir.join("vectors.bin.tmp"), dir.join("vectors.bin"))?;
        Ok(())
    }

    /// Brute-force cosine ranking: `k` (row index, similarity) pairs, best
    /// first. At knowledge-base scale the linear scan is the right index.
    pub fn top_k(&self, query: &[f32], k: usize) -> Vec<(usize, f32)> {
        let dims = self.manifest.dims;
        if query.len() != dims || self.manifest.count == 0 {
            return Vec::new();
        }
        let query_norm: f32 = query.iter().map(|value| value * value).sum::<f32>().sqrt();
        if query_norm == 0.0 {
            return Vec::new();
        }
        let mut ranked: Vec<(f32, usize)> = Vec::with_capacity(self.manifest.count);
        for row in 0..self.manifest.count {
            let vector = &self.vectors[row * dims..(row + 1) * dims];
            let dot: f32 = query.iter().zip(vector).map(|(q, v)| q * v).sum();
            let norm: f32 = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
            let score = if norm == 0.0 {
                0.0
            } else {
                dot / (query_norm * norm)
            };
            ranked.push((score, row));
        }
        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(k.max(1));
        ranked
            .into_iter()
            .map(|(score, row)| (row, score))
            .collect()
    }

    pub fn row(&self, row: usize) -> &[f32] {
        &self.vectors[row * self.manifest.dims..(row + 1) * self.manifest.dims]
    }
}

/// Per-user cache root (XDG_CACHE_HOME / LOCALAPPDATA / ~/.cache), shared by
/// the dense lane's index and model downloads.
fn user_cache_base() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        Some(PathBuf::from(xdg))
    } else {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache"))
    }
}

/// User-cache location for this Vault's dense lane, beside the lexical
/// index generations.
fn dense_dir(root: &Path) -> Option<PathBuf> {
    let base = user_cache_base()?;
    let vault = digest(root.to_string_lossy().as_bytes());
    Some(
        base.join("Notist")
            .join("indexes")
            .join(&vault[..16])
            .join("dense"),
    )
}

fn load_reuse_cache(dir: &Path) -> io::Result<HashMap<String, Vec<f32>>> {
    let raw = match fs::read(dir.join("reuse.bin")) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => return Err(error),
    };
    postcard::from_bytes(&raw).map_err(|error| io::Error::other(format!("reuse cache: {error}")))
}

fn store_reuse_cache(dir: &Path, cache: &HashMap<String, Vec<f32>>) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(
        dir.join("reuse.bin.tmp"),
        postcard::to_allocvec(cache).map_err(|error| io::Error::other(error.to_string()))?,
    )?;
    fs::rename(dir.join("reuse.bin.tmp"), dir.join("reuse.bin"))
}

/// Builds (or refreshes) the dense index: recut chunks, reuse every block
/// whose content hash already has a vector, embed only the misses.
pub fn build_dense_index(
    workspace: &WorkspaceSnapshot,
    identity: &SnapshotIdentity,
    config: &EmbeddingConfig,
) -> io::Result<DenseIndex> {
    let drafts = build_chunks(workspace, config)?;
    let mut embedder = EndpointEmbedder::new(config)?;
    let dir = dense_dir(workspace.root())
        .ok_or_else(|| io::Error::other("cannot locate the user cache directory"))?;
    fs::create_dir_all(&dir)?;
    let mut reuse: HashMap<String, Vec<f32>> = load_reuse_cache(&dir)?;

    // The vector width is learned, not declared: from a cached vector when
    // one exists, otherwise from the first embedded batch. A configured
    // `dims` seeds the expectation and every other width is rejected.
    let mut dims: Option<usize> = config.dims;
    let mut rows: Vec<Option<Vec<f32>>> = Vec::with_capacity(drafts.len());
    let mut misses = Vec::new();
    for (row, draft) in drafts.iter().enumerate() {
        match reuse.get(&draft.chunk.hash) {
            Some(vector) if !vector.is_empty() && dims.is_none_or(|d| vector.len() == d) => {
                dims.get_or_insert(vector.len());
                rows.push(Some(vector.clone()));
            }
            _ => {
                rows.push(None);
                misses.push(row);
            }
        }
    }

    const BATCH: usize = 64;
    let mut embedded_total = 0usize;
    for batch in misses.chunks(BATCH) {
        let texts: Vec<String> = batch.iter().map(|&row| drafts[row].text.clone()).collect();
        let out = embedder.embed(&texts)?;
        if out.len() != texts.len() {
            return Err(io::Error::other(format!(
                "embedder returned {} vectors for {} inputs",
                out.len(),
                texts.len()
            )));
        }
        for (slot, &row) in batch.iter().enumerate() {
            let vector = &out[slot];
            match dims {
                Some(d) if vector.len() != d => {
                    return Err(io::Error::other(format!(
                        "embedder returned {}-dim vectors, expected {d}",
                        vector.len()
                    )));
                }
                _ => dims.get_or_insert(vector.len()),
            };
            rows[row] = Some(vector.clone());
            reuse.insert(drafts[row].chunk.hash.clone(), vector.clone());
        }
        embedded_total += texts.len();
        eprintln!("dense: embedded {embedded_total}/{}", misses.len());
    }
    let dims = dims.unwrap_or(0);
    let mut vectors = Vec::with_capacity(drafts.len() * dims);
    for row in &rows {
        vectors.extend(row.as_deref().unwrap_or(&[]).iter().copied());
    }

    let live: HashSet<&str> = drafts
        .iter()
        .map(|draft| draft.chunk.hash.as_str())
        .collect();
    reuse.retain(|hash, _| live.contains(hash.as_str()));

    let manifest = DenseManifest {
        model_id: config.model_id(),
        dims,
        chunker_version: CHUNKER_VERSION.to_string(),
        granularity: config.granularity,
        context: config.context,
        source_fingerprint: identity.source_fingerprint.clone(),
        count: drafts.len(),
    };
    let index = DenseIndex {
        manifest,
        chunks: drafts.into_iter().map(|draft| draft.chunk).collect(),
        vectors,
    };
    index.store(&dir)?;
    store_reuse_cache(&dir, &reuse)?;
    Ok(index)
}

// ------------------------------------------------------------------ queries

/// `vsearch` input: the query text and how many hits to return. `k` bounds
/// the ranking itself — a ranked sample is the query's semantics, not an
/// output truncation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VectorSearchQuery {
    pub text: String,
    #[serde(default = "default_k")]
    pub k: usize,
}

fn default_k() -> usize {
    20
}

/// One ranked hit: similarity plus the read-addressable location.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VectorHit {
    pub score: f32,
    pub location: Location,
    pub heading_path: String,
    pub excerpt: String,
}

/// Dense-lane health for `index status`, computed without loading any
/// model: it only inspects config and the stored manifest.
pub fn dense_status(
    workspace: &WorkspaceSnapshot,
    identity: &SnapshotIdentity,
) -> Option<crate::query::DenseStatusRecord> {
    let config = EmbeddingConfig::from_vault_root(workspace.root()).ok()??;
    let dir = dense_dir(workspace.root())?;
    let record = match DenseIndex::load(&dir) {
        None => crate::query::DenseStatusRecord {
            state: "missing".into(),
            model: Some(config.model_id()),
            dims: None,
            unit_count: 0,
            message: Some("a vsearch query builds the dense lane on demand".into()),
        },
        Some(index) if index.manifest.matches(&config, identity) => {
            crate::query::DenseStatusRecord {
                state: "fresh".into(),
                model: Some(config.model_id()),
                dims: Some(index.manifest.dims),
                unit_count: index.manifest.count,
                message: None,
            }
        }
        Some(index) => crate::query::DenseStatusRecord {
            state: "stale".into(),
            model: Some(index.manifest.model_id),
            dims: Some(index.manifest.dims),
            unit_count: index.manifest.count,
            message: Some(
                "the stored dense index belongs to an older source set, model, or chunker".into(),
            ),
        },
    };
    Some(record)
}

/// Embeds the query and ranks the stored index. Rebuilds the index first
/// when it is missing or stale (blocking; embeddings for unchanged blocks
/// are reused, so refreshes cost only the changed blocks).
pub fn vector_search(
    workspace: &WorkspaceSnapshot,
    identity: &SnapshotIdentity,
    query: &VectorSearchQuery,
) -> Result<QueryResult<VectorHit>, ToolError> {
    let config = EmbeddingConfig::from_vault_root(workspace.root())
        .map_err(|error| {
            ToolError::new("embedding_config_error", error.to_string())
                .with_hint("fix the [embedding] table in Notist.toml")
        })?
        .ok_or_else(|| {
            ToolError::new(
                "embedding_not_configured",
                "the Vault declares no [embedding] table",
            )
            .with_hint(
                "add an [embedding] table to Notist.toml: `model` plus `endpoint` pointing \
                 at an OpenAI-compatible /v1 server (e.g. \"http://127.0.0.1:11434/v1\" for ollama)",
            )
        })?;
    let mut embedder = EndpointEmbedder::new(&config).map_err(|error| {
        ToolError::new("embedding_provider_unavailable", error.to_string())
            .retryable(
                "check the [embedding] endpoint/model configuration and retry; \
                 the lane ships no built-in inference — an OpenAI-compatible server must be running",
            )
    })?;
    let index = prepare_index(workspace, identity, &config).map_err(|error| {
        ToolError::new("dense_index_build_failed", error.to_string())
            .retryable("correct the error and retry; `notist index status` shows the dense lane")
    })?;
    search_index_with(&index, &mut embedder, identity, query)
}

/// Loads or rebuilds the dense index for `config`, reusing cached vectors
/// for every block whose content hash is unchanged.
pub fn prepare_index(
    workspace: &WorkspaceSnapshot,
    identity: &SnapshotIdentity,
    config: &EmbeddingConfig,
) -> io::Result<DenseIndex> {
    let dir = dense_dir(workspace.root())
        .ok_or_else(|| io::Error::other("cannot locate the user cache directory"))?;
    if let Some(index) = DenseIndex::load(&dir) {
        if index.manifest.matches(config, identity) {
            return Ok(index);
        }
    }
    build_dense_index(workspace, identity, config)
}

/// Ranks a prepared index against one query with a caller-held embedder
/// (bench harnesses load the model once per arm; the CLI path goes through
/// [`vector_search`]).
pub fn search_index_with(
    index: &DenseIndex,
    embedder: &mut dyn Embedder,
    identity: &SnapshotIdentity,
    query: &VectorSearchQuery,
) -> Result<QueryResult<VectorHit>, ToolError> {
    if query.text.trim().is_empty() {
        return Err(ToolError::new(
            "invalid_argument",
            "the query text is empty",
        ));
    }
    let embedded = embedder
        .embed(&[query.text.clone()])
        .map_err(|error| ToolError::new("embedding_failed", error.to_string()))?;
    let query_vector = embedded
        .into_iter()
        .next()
        .ok_or_else(|| ToolError::new("embedding_failed", "the embedder returned no vector"))?;
    if query_vector.len() != index.manifest.dims {
        return Err(ToolError::new(
            "embedding_failed",
            format!(
                "query vector is {}-dim but the index is {}-dim",
                query_vector.len(),
                index.manifest.dims
            ),
        ));
    }

    let records = index
        .top_k(&query_vector, query.k)
        .into_iter()
        .map(|(row, score)| {
            let chunk = &index.chunks[row];
            VectorHit {
                score,
                location: Location {
                    module: chunk.module.clone(),
                    relative_path: chunk.relative_path.clone(),
                    byte_range: ByteRange {
                        start: chunk.byte_range.0,
                        end: chunk.byte_range.1,
                    },
                    line_range: Some(LineRange {
                        start: chunk.line_range.0,
                        end: chunk.line_range.1,
                    }),
                    id: None,
                    source_fingerprint: identity.source_fingerprint.clone(),
                },
                heading_path: chunk.heading_path.clone(),
                excerpt: chunk.excerpt.clone(),
            }
        })
        .collect();

    Ok(QueryResult {
        snapshot: identity.clone(),
        records,
        search: None,
        hints: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_manifest(dir: &Path, text: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join("Notist.toml");
        fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn config_absent_when_no_table() {
        let dir = tempfile::tempdir().unwrap();
        write_manifest(dir.path(), "[plugins.shader]\npackage = \"shader\"\n");
        assert!(
            EmbeddingConfig::from_vault_root(dir.path())
                .unwrap()
                .is_none()
        );
        assert!(
            EmbeddingConfig::from_vault_root(tempfile::tempdir().unwrap().path())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn config_parses_endpoint_and_granularity() {
        let dir = tempfile::tempdir().unwrap();
        write_manifest(
            dir.path(),
            "[embedding]\nmodel = \"bge-small-zh-v1.5\"\nendpoint = \"http://127.0.0.1:11434/v1\"\ngranularity = \"paragraph\"\n",
        );
        let config = EmbeddingConfig::from_vault_root(dir.path())
            .unwrap()
            .unwrap();
        assert_eq!(config.model, "bge-small-zh-v1.5");
        assert_eq!(config.granularity, ChunkGranularity::Paragraph);
        assert_eq!(
            config.model_id(),
            "endpoint:http://127.0.0.1:11434/v1#bge-small-zh-v1.5"
        );
        assert!(config.dims.is_none());
    }

    #[test]
    fn config_requires_endpoint_and_positive_dims() {
        // The lane ships no built-in inference: a model alone is invalid.
        let dir = tempfile::tempdir().unwrap();
        write_manifest(dir.path(), "[embedding]\nmodel = \"bge-small-zh-v1.5\"\n");
        assert!(EmbeddingConfig::from_vault_root(dir.path()).is_err());

        let dir = tempfile::tempdir().unwrap();
        write_manifest(
            dir.path(),
            "[embedding]\nmodel = \"m\"\nendpoint = \"http://127.0.0.1:11434/v1\"\ndims = 0\n",
        );
        assert!(EmbeddingConfig::from_vault_root(dir.path()).is_err());
    }

    #[test]
    fn line_range_and_excerpt() {
        let text = "first\nsecond\n\nthird line\n";
        assert_eq!(line_range_of(text, (0, 6)), (1, 1));
        assert_eq!(line_range_of(text, (7, 13)), (2, 2));
        assert_eq!(line_range_of(text, (14, 24)), (4, 4));
        assert_eq!(
            excerpt_of("\n\n  leading blank picks next  \n"),
            "leading blank picks next"
        );
        let long = "x".repeat(200);
        assert_eq!(excerpt_of(&long).chars().count(), 97);
        assert!(excerpt_of(&long).ends_with('…'));
    }

    #[test]
    fn paragraph_drafts_cut_on_blank_lines_and_skip_comment_lines() {
        let mut source = String::new();
        source.push_str("= Title\n"); // line 1 (outside the section given below)
        source.push_str("== Section\n"); // line 2
        source.push_str("para one alpha\n"); // line 3
        source.push_str("para one beta\n"); // line 4
        source.push_str("\n"); // line 5 (blank separator)
        source.push_str("// secret\n"); // line 6 (comment line)
        source.push_str("para two\n"); // line 7
        let section_start = source.find("== Section").unwrap();
        let section = TextRange::new(section_start, source.len());
        // The comment spans its whole line.
        let comment_start = source.find("// secret").unwrap();
        let comments = vec![TextRange::new(comment_start, comment_start + 9)];

        let drafts = paragraph_drafts(
            "builtin:test",
            "vault::demo",
            Path::new("demo.not"),
            "Title/Section",
            section,
            &source,
            &comments,
        );
        assert_eq!(drafts.len(), 2);
        let (one, two) = (&drafts[0], &drafts[1]);
        assert_eq!(one.text, "Title/Section\npara one alpha\npara one beta");
        assert_eq!(two.text, "Title/Section\npara two");
        // byte ranges resolve back to the source lines
        assert_eq!(
            &source[one.chunk.byte_range.0..one.chunk.byte_range.1],
            "para one alpha\npara one beta"
        );
        assert_eq!(
            &source[two.chunk.byte_range.0..two.chunk.byte_range.1],
            "para two"
        );
        // line ranges are 1-based inclusive against the whole source
        assert_eq!(one.chunk.line_range, (3, 4));
        assert_eq!(two.chunk.line_range, (7, 7));
        assert!(two.chunk.excerpt.starts_with("para two"));
        assert_eq!(
            one.chunk.hash,
            content_hash(
                "builtin:test",
                ChunkGranularity::Paragraph,
                ChunkContext::None,
                "Title/Section",
                &one.text
            )
        );
        assert_ne!(one.chunk.hash, two.chunk.hash);
    }

    #[test]
    fn hashes_differ_by_content_and_model() {
        let a = content_hash(
            "builtin:m",
            ChunkGranularity::Section,
            ChunkContext::None,
            "H",
            "body",
        );
        let b = content_hash(
            "builtin:m",
            ChunkGranularity::Section,
            ChunkContext::None,
            "H",
            "body2",
        );
        let c = content_hash(
            "builtin:other",
            ChunkGranularity::Section,
            ChunkContext::None,
            "H",
            "body",
        );
        let d = content_hash(
            "builtin:m",
            ChunkGranularity::Paragraph,
            ChunkContext::None,
            "H",
            "body",
        );
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }

    #[test]
    fn dense_index_roundtrip_and_ranking() {
        let dir = tempfile::tempdir().unwrap();
        let chunk = |name: &str| VectorChunk {
            module: "vault::demo".into(),
            relative_path: PathBuf::from(format!("{name}.not")),
            heading_path: name.into(),
            byte_range: (0, 1),
            line_range: (1, 1),
            excerpt: "x".into(),
            hash: name.into(),
        };
        let index = DenseIndex {
            manifest: DenseManifest {
                model_id: "test".into(),
                dims: 2,
                chunker_version: CHUNKER_VERSION.into(),
                granularity: ChunkGranularity::Section,
                context: ChunkContext::None,
                source_fingerprint: "fp".into(),
                count: 3,
            },
            chunks: vec![chunk("near"), chunk("far"), chunk("mid")],
            // near=[1,0] (cos 1.0 to query), far=[-1,0], mid=[0.707,0.707]
            vectors: vec![1.0, 0.0, -1.0, 0.0, 0.7071, 0.7071],
        };
        index.store(dir.path()).unwrap();
        let loaded = DenseIndex::load(dir.path()).unwrap();
        assert_eq!(loaded.manifest.count, 3);
        assert_eq!(loaded.manifest.dims, 2);
        let ranked = loaded.top_k(&[1.0, 0.0], 2);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].0, 0);
        assert!((ranked[0].1 - 1.0).abs() < 1e-5);
        assert_eq!(ranked[1].0, 2);
        assert!(loaded.top_k(&[0.0, 0.0], 5).is_empty());
        assert!(loaded.top_k(&[1.0, 0.0, 0.0], 5).is_empty());
    }
}
