# Frozen Eval Corpora

Both trees are **explicitly frozen snapshots**. Upstream docs changes do not
affect them; re-syncing is a deliberate act with its own protocol (see
`docs/designs/host/cli-agent-eval-framework.not`, section "语料政策").

| tree | derived from | content digest (sha256-16) |
| --- | --- | --- |
| `notist/`   | main repo docs+plugins @ commit **3ec331c** (rewrite-stable pin; was 1666adc before the main history rewrite) (full vault incl. `ai/`; mermaid/shader packages bundled so bare calls register), plus derived `@![kind,status]` attrs on every module lacking them | `ed6c54bda9027c05` |
| `markdown/` | that tree through `../tools/build_corpus_v2.py convert` (strict Markdown v2: ATX headings, `**strong**`, digit-ordered lists, frontmatter, `<a id>` anchors) | `a77e7cc5623659d7` |

The converter file and this pointer are updated together in a single freeze
commit; treat "the converter at this repo's freeze commit" as its pin.
`.corpus-source-commit` inside each tree records the pin but is excluded
from the digest (pointer is metadata, not content); the rewrite-immune
digests above are the authoritative ones.

Digest = sha256 over the concatenation of every file's relative path bytes and
content bytes, sorted by path (see "corpus_digest" in `../tools/run_eval.py`;
the same value is stamped into every run's `manifest.json`).

## Extraction rules applied (notist)

- full vault: `ai/` research entries are INCLUDED since v2
- excluded: `.obsidian/`, `AGENTS.md`; NOTIST.toml declares the two bundled
  plugin packages with repo-relative paths so `mermaid`/`shader` register
- 101 sources; every module gains derived `@![kind=..., status=...]`
  attributes (`status="archived"` for dated research) merged into existing
  leading attribute lines when present
- the vault checks CLEAN at freeze time (2026-08-27): upstream gaps fixed on
  main in commits 5a17c2a + 3ec331c before this freeze (pre-rewrite names 981f7f4/1666adc), per the re-sync rule

Markdown v1 strictness defects are resolved in v2: headings become real ATX
`#`, single-star strong becomes `**strong**`, ordered `+` items become digit
lists; a validator run over v2 reports zero leaks.

## Re-sync procedure

1. `git archive <new-commit> -- docs | tar -x` into a fresh `notist/`
2. apply the exclusions above, empty out `Notist.toml`
3. regenerate `markdown/` with the pinned converter version
4. update this table (source commit, converter commit, digests)
5. re-run the baseline grid — a new anchor is mandatory before any further
   iteration comparisons may reference the new corpus pointer
