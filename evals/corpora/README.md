# Frozen Eval Corpora

Both trees are **explicitly frozen snapshots**. Upstream docs changes do not
affect them; re-syncing is a deliberate act with its own protocol (see
`docs/designs/host/cli-agent-eval-framework.not`, section "语料政策").

| tree | derived from | content digest (sha256-16) |
| --- | --- | --- |
| `notist/`   | main repo `docs` @ commit **c86a090** via `git archive c86a090 -- docs` | `29ee536fd1793f67` |
| `markdown/` | `notist/` through `../tools/convert_notist_to_md.py` @ commit **29d757b** | `cc01510f6a3dbc3d` |

Digest = sha256 over the concatenation of every file's relative path bytes and
content bytes, sorted by path (see "corpus_digest" in `../tools/run_eval.py`;
the same value is stamped into every run's `manifest.json`).

## Extraction rules applied (notist)

- excluded: `ai/` (dated research archive), `.obsidian/`, `AGENTS.md`
- `Notist.toml` replaced with an empty file to avoid plugin-path resolution
- known frozen defect, intentionally not patched in place:
  `README.not` still references the removed `#<ai>` target and produces one
  error-level unresolved-module diagnostic

## Re-sync procedure

1. `git archive <new-commit> -- docs | tar -x` into a fresh `notist/`
2. apply the exclusions above, empty out `Notist.toml`
3. regenerate `markdown/` with the pinned converter version
4. update this table (source commit, converter commit, digests)
5. re-run the baseline grid — a new anchor is mandatory before any further
   iteration comparisons may reference the new corpus pointer
