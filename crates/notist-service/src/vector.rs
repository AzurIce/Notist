//! Dense (vector) retrieval over a Vault: structural chunks, an embedder
//! abstraction, a rebuildable local vector file, and brute-force cosine
//! queries. This is the `notist vsearch` experiment lane — it shares no
//! contract with the lexical search family and touches nothing there.

#[cfg(not(any(feature = "dense-download", feature = "dense-system")))]
compile_error!("select exactly one dense backend feature: dense-download or dense-system");

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
}

impl ChunkGranularity {
    fn parse(value: &str) -> io::Result<Self> {
        match value {
            "section" => Ok(Self::Section),
            "paragraph" => Ok(Self::Paragraph),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "unknown embedding granularity `{other}` (expected `section` or `paragraph`)"
                ),
            )),
        }
    }
}

/// `[embedding]` table of the Vault `Notist.toml`. Absence of the table
/// simply disables the dense lane.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddingConfig {
    /// Built-in fastembed model name (`bge-m3`, `bge-small-zh-v1.5`, …), or
    /// the model identifier forwarded to `endpoint`.
    pub model: String,
    /// OpenAI-compatible `/v1` base URL. When set, embeddings are served
    /// over HTTP instead of the in-process ONNX runtime.
    pub endpoint: Option<String>,
    /// Vector width; required for `endpoint`, derived by probing built-ins.
    pub dims: Option<usize>,
    /// Environment variable carrying the endpoint bearer token.
    pub api_key_env: Option<String>,
    pub granularity: ChunkGranularity,
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
            .map(str::to_string);
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
        let config = Self {
            model,
            endpoint,
            dims,
            api_key_env,
            granularity,
        };
        config.validate()?;
        Ok(Some(config))
    }

    fn validate(&self) -> io::Result<()> {
        if self.endpoint.is_none() {
            parse_builtin_model(&self.model)?;
        } else if self.dims.is_none() || self.dims == Some(0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "[embedding] endpoint provider requires an integer `dims`",
            ));
        }
        Ok(())
    }

    /// Stable identity of the embedding function; any change invalidates the
    /// stored vectors (different models produce incomparable spaces).
    pub fn model_id(&self) -> String {
        match &self.endpoint {
            None => format!("builtin:{}", self.model),
            Some(endpoint) => format!("endpoint:{}#{}", endpoint.trim_end_matches('/'), self.model),
        }
    }
}

/// The embedding function abstraction: one implementation per provider
/// shape. `dims` is known up front so the vector file can be sized before
/// any batch round-trip.
pub trait Embedder {
    fn model_id(&self) -> String;
    fn dims(&self) -> usize;
    /// Embeds a batch of texts, returning one vector per input in order.
    fn embed(&mut self, texts: &[String]) -> io::Result<Vec<Vec<f32>>>;
}

// ---------------------------------------------------------------- built-in

/// In-process ONNX inference via `fastembed`. The model itself is runtime
/// state: first use downloads it from HuggingFace into the user cache.
pub struct BuiltinEmbedder {
    inner: fastembed::TextEmbedding,
    id: String,
    dims: usize,
}

fn parse_builtin_model(model: &str) -> io::Result<fastembed::EmbeddingModel> {
    use fastembed::EmbeddingModel as M;
    let variant = match model {
        "bge-m3" => M::BGEM3,
        "bge-small-zh-v1.5" => M::BGESmallZHV15,
        "bge-large-zh-v1.5" => M::BGELargeZHV15,
        "bge-small-en-v1.5" => M::BGESmallENV15,
        "bge-base-en-v1.5" => M::BGEBaseENV15,
        "bge-large-en-v1.5" => M::BGELargeENV15,
        "multilingual-e5-small" => M::MultilingualE5Small,
        "multilingual-e5-base" => M::MultilingualE5Base,
        "multilingual-e5-large" => M::MultilingualE5Large,
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "unknown built-in embedding model `{other}` (supported: bge-m3, \
                     bge-small-zh-v1.5, bge-large-zh-v1.5, bge-small/base/large-en-v1.5, \
                     multilingual-e5-small/base/large)"
                ),
            ));
        }
    };
    Ok(variant)
}

impl BuiltinEmbedder {
    pub fn new(model: &str) -> io::Result<Self> {
        // fastembed's default cache directory is `./.fastembed_cache` in the
        // current working directory; pin it to the user cache so models are
        // downloaded once and never litter the Vault or the shell's cwd.
        let model_cache = user_cache_base()
            .map(|base| base.join("Notist").join("models"))
            .ok_or_else(|| io::Error::other("cannot locate the user cache directory"))?;
        let options = fastembed::TextInitOptions::new(parse_builtin_model(model)?)
            .with_show_download_progress(false)
            .with_cache_dir(model_cache)
            .with_intra_threads(
                std::thread::available_parallelism()
                    .map(|parallelism| parallelism.get())
                    .unwrap_or(4),
            );
        let mut inner = fastembed::TextEmbedding::try_new(options)
            .map_err(|error| io::Error::other(format!("embedding model load failed: {error}")))?;
        // The vector width follows from the checkpoint; probe it once so the
        // store can be sized without a per-model lookup table.
        let probe = inner
            .embed(vec!["dimension probe".to_string()], None)
            .map_err(|error| io::Error::other(format!("embedding probe failed: {error}")))?;
        let dims = probe
            .first()
            .map(|vector| vector.len())
            .ok_or_else(|| io::Error::other("embedding probe returned no vector"))?;
        Ok(Self {
            inner,
            id: format!("builtin:{model}"),
            dims,
        })
    }
}

impl Embedder for BuiltinEmbedder {
    fn model_id(&self) -> String {
        self.id.clone()
    }

    fn dims(&self) -> usize {
        self.dims
    }

    fn embed(&mut self, texts: &[String]) -> io::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        self.inner
            .embed(texts.to_vec(), None)
            .map_err(|error| io::Error::other(format!("embedding failed: {error}")))
    }
}

// ----------------------------------------------------------------- endpoint

/// OpenAI-compatible `/v1/embeddings` client (ollama, llama.cpp, vLLM, cloud
/// providers all speak this shape).
pub struct EndpointEmbedder {
    agent: ureq::Agent,
    url: String,
    model: String,
    auth: Option<String>,
    id: String,
    dims: usize,
}

const ENDPOINT_BATCH: usize = 32;

impl EndpointEmbedder {
    pub fn new(config: &EmbeddingConfig) -> io::Result<Self> {
        let endpoint = config
            .endpoint
            .as_deref()
            .ok_or_else(|| io::Error::other("endpoint provider requires [embedding].endpoint"))?;
        let dims = config
            .dims
            .ok_or_else(|| io::Error::other("endpoint provider requires [embedding].dims"))?;
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
            url: format!("{}/embeddings", endpoint.trim_end_matches('/')),
            model: config.model.clone(),
            auth,
            id: config.model_id(),
            dims,
        })
    }
}

impl Embedder for EndpointEmbedder {
    fn model_id(&self) -> String {
        self.id.clone()
    }

    fn dims(&self) -> usize {
        self.dims
    }

    fn embed(&mut self, texts: &[String]) -> io::Result<Vec<Vec<f32>>> {
        let mut vectors = Vec::with_capacity(texts.len());
        for batch in texts.chunks(ENDPOINT_BATCH) {
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

/// Builds the provider selected by the config.
pub fn make_embedder(config: &EmbeddingConfig) -> io::Result<Box<dyn Embedder>> {
    match &config.endpoint {
        None => Ok(Box::new(BuiltinEmbedder::new(&config.model)?)),
        Some(_) => Ok(Box::new(EndpointEmbedder::new(config)?)),
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

impl ChunkGranularity {
    fn as_str(self) -> &'static str {
        match self {
            Self::Section => "section",
            Self::Paragraph => "paragraph",
        }
    }
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
                module,
                relative,
                chain,
                (start, end),
                source,
                text,
            )
        })
        .collect()
}

/// Cuts every source module into embeddable blocks on the evaluated Item
/// tree. Section granularity takes whole heading sections; paragraph
/// granularity splits sections at blank lines and prefixes each block with
/// its heading chain so the context survives the cut.
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
        let mut scopes = Vec::new();
        crate::query::collect_section_scopes(&structured.tree.roots, &mut scopes);
        scopes.sort_by_key(|(_, range)| range.start);
        let comments = crate::query::comment_ranges(&source.text);
        let relative = crate::query::relative_path(workspace.root(), &source.canonical_path);
        let module_name = module.logical_path.to_string();
        for (chain, range) in scopes {
            match config.granularity {
                ChunkGranularity::Section => {
                    let text = crate::query::text_excluding(&source.text, range, &comments);
                    if text.trim().is_empty() {
                        continue;
                    }
                    let byte_range = (range.start, range.end);
                    drafts.push(make_draft(
                        &model_id,
                        config.granularity,
                        &module_name,
                        &relative,
                        &chain,
                        byte_range,
                        &source.text,
                        text,
                    ));
                }
                ChunkGranularity::Paragraph => {
                    drafts.extend(paragraph_drafts(
                        &model_id,
                        &module_name,
                        &relative,
                        &chain,
                        range,
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
    module: &str,
    relative: &Path,
    chain: &str,
    byte_range: (usize, usize),
    source: &str,
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
            excerpt: excerpt_of(&text),
            hash: content_hash(model_id, granularity, chain, &text),
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
    pub source_fingerprint: String,
    pub count: usize,
}

impl DenseManifest {
    fn matches(&self, config: &EmbeddingConfig, identity: &SnapshotIdentity) -> bool {
        self.model_id == config.model_id()
            && self.chunker_version == CHUNKER_VERSION
            && self.granularity == config.granularity
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
    let mut embedder = make_embedder(config)?;
    let dims = embedder.dims();
    let dir = dense_dir(workspace.root())
        .ok_or_else(|| io::Error::other("cannot locate the user cache directory"))?;
    fs::create_dir_all(&dir)?;
    let mut reuse: HashMap<String, Vec<f32>> = load_reuse_cache(&dir)?;

    let mut vectors = vec![0.0f32; drafts.len() * dims];
    let mut misses = Vec::new();
    for (row, draft) in drafts.iter().enumerate() {
        match reuse.get(&draft.chunk.hash) {
            Some(vector) if vector.len() == dims => {
                vectors[row * dims..(row + 1) * dims].copy_from_slice(vector);
            }
            _ => misses.push(row),
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
            if vector.len() != dims {
                return Err(io::Error::other(format!(
                    "embedder returned {}-dim vectors, expected {dims}",
                    vector.len()
                )));
            }
            vectors[row * dims..(row + 1) * dims].copy_from_slice(vector);
            reuse.insert(drafts[row].chunk.hash.clone(), vector.clone());
        }
        embedded_total += texts.len();
        eprintln!("dense: embedded {embedded_total}/{}", misses.len());
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
            message: Some("run `notist index rebuild` or issue a vsearch query".into()),
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
    if query.text.trim().is_empty() {
        return Err(ToolError::new(
            "invalid_argument",
            "the query text is empty",
        ));
    }
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
            .with_hint("add an [embedding] table with at least `model = \"bge-m3\"` to Notist.toml")
        })?;

    let dir = dense_dir(workspace.root())
        .ok_or_else(|| ToolError::new("internal", "cannot locate the user cache directory"))?;
    let fresh = match DenseIndex::load(&dir) {
        Some(index) => index.manifest.matches(&config, identity),
        None => false,
    };
    let index = if fresh {
        DenseIndex::load(&dir).expect("checked fresh above")
    } else {
        build_dense_index(workspace, identity, &config).map_err(|error| {
            ToolError::new("dense_index_build_failed", error.to_string()).retryable(
                "correct the error and retry; `notist index status` shows the dense lane",
            )
        })?
    };

    let mut embedder = make_embedder(&config).map_err(|error| {
        ToolError::new("embedding_provider_unavailable", error.to_string())
            .retryable("check the [embedding] endpoint/model configuration and retry")
    })?;
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
    fn config_parses_builtin_and_granularity() {
        let dir = tempfile::tempdir().unwrap();
        write_manifest(
            dir.path(),
            "[embedding]\nmodel = \"bge-m3\"\ngranularity = \"paragraph\"\n",
        );
        let config = EmbeddingConfig::from_vault_root(dir.path())
            .unwrap()
            .unwrap();
        assert_eq!(config.model, "bge-m3");
        assert_eq!(config.granularity, ChunkGranularity::Paragraph);
        assert_eq!(config.model_id(), "builtin:bge-m3");
        assert!(config.endpoint.is_none());
    }

    #[test]
    fn config_rejects_unknown_model_and_incomplete_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        write_manifest(dir.path(), "[embedding]\nmodel = \"nope\"\n");
        assert!(EmbeddingConfig::from_vault_root(dir.path()).is_err());

        let dir = tempfile::tempdir().unwrap();
        write_manifest(
            dir.path(),
            "[embedding]\nmodel = \"qwen3-embedding:0.6b\"\nendpoint = \"http://127.0.0.1:11434/v1\"\n",
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
        assert!(two.chunk.excerpt.starts_with("Title/Section"));
        assert_eq!(
            one.chunk.hash,
            content_hash(
                "builtin:test",
                ChunkGranularity::Paragraph,
                "Title/Section",
                &one.text
            )
        );
        assert_ne!(one.chunk.hash, two.chunk.hash);
    }

    #[test]
    fn hashes_differ_by_content_and_model() {
        let a = content_hash("builtin:m", ChunkGranularity::Section, "H", "body");
        let b = content_hash("builtin:m", ChunkGranularity::Section, "H", "body2");
        let c = content_hash("builtin:other", ChunkGranularity::Section, "H", "body");
        let d = content_hash("builtin:m", ChunkGranularity::Paragraph, "H", "body");
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
