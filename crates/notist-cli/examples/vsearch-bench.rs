//! Experiment harness for the dense (vsearch) lane: runs a labeled query set
//! against embedding-server arms and emits one JSON blob of rankings for
//! offline scoring (with `experiments/vsearch-bench/score.py`). Not part of
//! the CLI contract — this is experiment tooling.
//!
//! Usage: vsearch-bench <VAULT> <QUERIES.json> <OUT.json> [--only SUBSTR]

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
    /// OpenAI-compatible `/v1` base URL of the embedding server.
    endpoint: &'static str,
}

fn arms() -> Vec<Arm> {
    use ChunkContext::None as NoContext;
    use ChunkGranularity::Section;
    // Same model weights (bge-small-zh-v1.5 f16 GGUF) across servers:
    // 8934 = llama-server `-ngl 0 -t 4` (CPU), 8935 = the same binary with
    // `-ngl 99` (CUDA), 11434 = ollama. Isolates runtime + hardware cost
    // from chunking quality; the chunking/context factor study lives in the
    // experiment record.
    vec![
        Arm {
            name: "llama-cpu-sec",
            model: "bge-small-zh-v1.5",
            granularity: Section,
            context: NoContext,
            endpoint: "http://127.0.0.1:8934/v1",
        },
        Arm {
            name: "llama-gpu-sec",
            model: "bge-small-zh-v1.5",
            granularity: Section,
            context: NoContext,
            endpoint: "http://127.0.0.1:8935/v1",
        },
        Arm {
            name: "ollama-gpu-sec",
            model: "bge-small-zh-v1.5",
            granularity: Section,
            context: NoContext,
            endpoint: "http://127.0.0.1:11434/v1",
        },
        // 2026-09-11 embedding-model shootout on the LAN Mac mini M4: LM
        // Studio headless, OpenAI /v1/embeddings. All arms run the
        // 2026-09-08-winning recipe (section + chain-attrs) so model quality
        // is the only variable; dims are probed from the first response.
        Arm {
            name: "lmstudio-nomic-attrs",
            model: "text-embedding-nomic-embed-text-v1.5",
            granularity: Section,
            context: ChunkContext::ChainAttrs,
            endpoint: "http://192.168.2.11:1234/v1",
        },
        Arm {
            name: "lmstudio-bgem3-attrs",
            model: "text-embedding-bge-m3",
            granularity: Section,
            context: ChunkContext::ChainAttrs,
            endpoint: "http://192.168.2.11:1234/v1",
        },
        Arm {
            name: "lmstudio-qwen3-06b-attrs",
            model: "text-embedding-qwen3-embedding-0.6b",
            granularity: Section,
            context: ChunkContext::ChainAttrs,
            endpoint: "http://192.168.2.11:1234/v1",
        },
        Arm {
            name: "lmstudio-embgemma-attrs",
            model: "text-embedding-embeddinggemma-300m-qat",
            granularity: Section,
            context: ChunkContext::ChainAttrs,
            endpoint: "http://192.168.2.11:1234/v1",
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
    let outcome = service.with_snapshot_identity(view_id, |workspace, identity| {
        for arm in arms() {
            if let Some(only) = &args.only
                && !arm.name.contains(only.as_str())
            {
                continue;
            }
            eprintln!("=== arm {} ===", arm.name);
            let config = EmbeddingConfig {
                model: arm.model.to_string(),
                endpoint: arm.endpoint.to_string(),
                dims: None,
                api_key_env: None,
                granularity: arm.granularity,
                context: arm.context,
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
            let mut embedder = vector::EndpointEmbedder::new(&config)?;
            eprintln!(
                "arm {}: {} chunks (dims {}), built in {} ms",
                arm.name, index.manifest.count, index.manifest.dims, build_ms
            );

            let mut records = Vec::new();
            for query in &queries {
                let started = Instant::now();
                let result = vector::search_index_with(
                    &index,
                    &mut embedder,
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
    });
    let (_, outcome) = outcome?;
    outcome?;

    std::fs::write(&args.output, serde_json::to_vec_pretty(&results)?)?;
    eprintln!("wrote {}", args.output.display());
    Ok(())
}
