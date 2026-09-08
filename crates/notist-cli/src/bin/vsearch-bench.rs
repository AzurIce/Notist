//! Experiment harness for the dense (vsearch) lane: runs a labeled query set
//! against multiple chunking / context / model arms and emits one JSON blob
//! of rankings for offline scoring. Not part of the CLI contract — this is
//! worktree experiment tooling.
//!
//! Usage: vsearch-bench <VAULT> <QUERIES.json> <OUT.json>

use std::path::PathBuf;
use std::time::Instant;

use clap::Parser;
use notist_analysis::resolve_vault_root;
use notist_service::vector::{
    self, ChunkContext, ChunkGranularity, EmbeddingConfig, VectorSearchQuery,
};
use notist_service::{CoreRequest, CoreResponse, NotistService, ProtocolViewKind};
use serde::{Deserialize, Serialize};

#[derive(Parser)]
struct Args {
    vault: PathBuf,
    queries: PathBuf,
    output: PathBuf,
    /// Run only arms whose name contains this substring.
    #[arg(long)]
    only: Option<String>,
}

#[derive(Deserialize)]
struct BenchQuery {
    id: String,
    kind: String,
    text: String,
}

#[derive(Serialize)]
struct Hit {
    path: String,
    chain: String,
    score: f32,
}

#[derive(Serialize)]
struct QueryResultRecord {
    id: String,
    kind: String,
    ms: u128,
    hits: Vec<Hit>,
}

#[derive(Serialize)]
struct ArmResult {
    arm: String,
    model: String,
    dims: usize,
    chunk_count: usize,
    build_ms: u128,
    queries: Vec<QueryResultRecord>,
}

struct Arm {
    name: &'static str,
    model: &'static str,
    granularity: ChunkGranularity,
    context: ChunkContext,
    /// When set, embeddings come from an OpenAI-compatible endpoint instead
    /// of the in-process ONNX runtime (llama-server comparison arms).
    endpoint: Option<&'static str>,
}

fn arms() -> Vec<Arm> {
    use ChunkContext::{ChainAttrs, None as NoContext};
    use ChunkGranularity::{Paragraph, Scope, Section};
    vec![
        Arm {
            name: "s-sec-none",
            model: "bge-small-zh-v1.5",
            granularity: Section,
            context: NoContext,
            endpoint: None,
        },
        Arm {
            name: "s-sec-chain",
            model: "bge-small-zh-v1.5",
            granularity: Section,
            context: ChunkContext::Chain,
            endpoint: None,
        },
        Arm {
            name: "s-sec-attrs",
            model: "bge-small-zh-v1.5",
            granularity: Section,
            context: ChainAttrs,
            endpoint: None,
        },
        Arm {
            name: "s-para",
            model: "bge-small-zh-v1.5",
            granularity: Paragraph,
            context: NoContext,
            endpoint: None,
        },
        Arm {
            name: "s-scope-none",
            model: "bge-small-zh-v1.5",
            granularity: Scope,
            context: NoContext,
            endpoint: None,
        },
        Arm {
            name: "s-scope-attrs",
            model: "bge-small-zh-v1.5",
            granularity: Scope,
            context: ChainAttrs,
            endpoint: None,
        },
        // Same model weights served by llama-server (bge-small-zh-v1.5-f16.gguf):
        // 8934 = llama-server CPU build (-ngl 0, -t 4); 11434 = ollama serving the
        // same f16 GGUF on the RTX 4070 (CUDA). Isolates
        // runtime + hardware cost from chunking quality.
        Arm {
            name: "llama-cpu-sec",
            model: "bge-small-zh-v1.5",
            granularity: Section,
            context: NoContext,
            endpoint: Some("http://127.0.0.1:8934/v1"),
        },
        Arm {
            name: "ollama-gpu-sec",
            model: "bge-small-zh-v1.5",
            granularity: Section,
            context: NoContext,
            endpoint: Some("http://127.0.0.1:11434/v1"),
        },
    ]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let root = resolve_vault_root(&args.vault)?;
    let queries: Vec<BenchQuery> = serde_json::from_str(&std::fs::read_to_string(&args.queries)?)?;
    eprintln!("queries: {}", queries.len());

    let service = NotistService::for_root(&root)?;
    let reply = service.execute(CoreRequest::OpenView {
        root: root.clone(),
        kind: ProtocolViewKind::Disk,
    })?;
    let CoreResponse::Opened { view_id, .. } = reply.response else {
        return Err("unexpected open-view response".into());
    };

    let mut results: Vec<ArmResult> = Vec::new();
    let (_, outcome) = service.with_snapshot_identity(view_id, |workspace, identity| {
        for arm in arms() {
            if let Some(only) = &args.only
                && !arm.name.contains(only.as_str())
            {
                continue;
            }
            eprintln!("=== arm {} ===", arm.name);
            let config = EmbeddingConfig {
                model: arm.model.to_string(),
                endpoint: arm.endpoint.map(str::to_string),
                dims: arm.endpoint.map(|_| 512),
                api_key_env: None,
                granularity: arm.granularity,
                context: arm.context,
                threads: None,
            };
            let build_started = Instant::now();
            let index = match vector::prepare_index(workspace, identity, &config) {
                Ok(index) => index,
                Err(error) => {
                    eprintln!("arm {} FAILED: {error}", arm.name);
                    return Ok::<(), Box<dyn std::error::Error>>(());
                }
            };
            let build_ms = build_started.elapsed().as_millis();
            let mut embedder = vector::make_embedder(&config)?;
            eprintln!(
                "arm {}: {} chunks (dims {}), built in {} ms",
                arm.name, index.manifest.count, index.manifest.dims, build_ms
            );

            let mut records = Vec::new();
            for query in &queries {
                let started = Instant::now();
                let result = vector::search_index_with(
                    &index,
                    embedder.as_mut(),
                    identity,
                    &VectorSearchQuery {
                        text: query.text.clone(),
                        k: 10,
                    },
                )
                .map_err(|error| {
                    format!("arm {} query {}: {}", arm.name, query.id, error.message)
                })?;
                let hits = result
                    .records
                    .iter()
                    .map(|hit| Hit {
                        path: hit.location.relative_path.to_string_lossy().to_string(),
                        chain: hit.heading_path.clone(),
                        score: hit.score,
                    })
                    .collect();
                records.push(QueryResultRecord {
                    id: query.id.clone(),
                    kind: query.kind.clone(),
                    ms: started.elapsed().as_millis(),
                    hits,
                });
            }
            results.push(ArmResult {
                arm: arm.name.to_string(),
                model: arm.model.to_string(),
                dims: index.manifest.dims,
                chunk_count: index.manifest.count,
                build_ms,
                queries: records,
            });
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;
    outcome?;

    std::fs::write(&args.output, serde_json::to_vec_pretty(&results)?)?;
    eprintln!("wrote {}", args.output.display());
    Ok(())
}
