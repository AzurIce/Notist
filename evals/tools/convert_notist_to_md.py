#!/usr/bin/env python3
"""Convert a frozen notist docs corpus into an equivalent Markdown corpus.

Scope decisions (recorded for reproducibility):
- Subset: official reference docs only (README, intro, grammar, types, functions,
  cheatsheet, cli, plugins, example/, obsidian-notist/, designs/, todo.not).
  docs/ai research notes are excluded on purpose; they are a dated archive.
- Markup sugar (headings/emphasis/lists/tables/rules/fences) passes through
  unchanged - it is already Markdown-compatible.
- Wiki references `#<target>` become `[text](relative.md#anchor)` links when
  resolvable to a converted module; otherwise kept as literal `<target>` text.
- Annotations keep their authored content and drop the metadata tail
  (`#[X]@tag,...` -> `X`); block prefixes `@[...]` / module props `@![...]`
  are dropped as standalone lines.
- Bare constructor calls outside fences are fenced as ```not blocks verbatim
  (bracket-balanced scan); their bodies stay greppable for graders.
"""

import re
import shutil
import sys
from pathlib import Path

REF_RE = re.compile(r"#<([A-Za-z0-9_./:\-\u4e00-\u9fff]+)>")
ANN_POSTFIX_RE = re.compile(r"\]@([A-Za-z0-9_.,=\s\"'()/:\u4e00-\u9fff-]+)(?=\s|$)")
BLOCK_ANN_RE = re.compile(r"^@\[.*\]\s*$")
MODULE_ANN_RE = re.compile(r"^@!\[.*\]\s*$")

OFFICIAL = [
    "README.not", "intro.not", "grammar.not", "types.not", "functions.not",
    "cheatsheet.not", "cli.not", "plugins.not", "todo.not",
    "example", "obsidian-notist", "designs",
]


def module_id(rel: Path) -> str:
    """docs/guide/README.not -> vault::guide ; docs/guide/setup.not -> vault::guide::setup"""
    parts = list(rel.with_suffix("").parts)
    if parts[-1] == "README":
        parts = parts[:-1] or ["vault"]
    else:
        parts = ["vault"] + parts
    return "::".join(parts)


def resolve_ref(target: str, cur_mid: str, modules: dict[str, Path], cur_dir: Path,
                root: Path) -> tuple[str, bool]:
    """Return (markdown link target url relative to cur_dir, ok)."""
    t = target
    scheme = ""
    if t.startswith("vault::"):
        t = t[len("vault::"):]
        base_parts = []
    elif t.startswith("self::"):
        base_parts = cur_mid.split("::")[1:]
        t = t[len("self::"):]
    elif t.startswith("super::"):
        base_parts = cur_mid.split("::")[1:-1]
        t = t[len("super::"):]
        while t.startswith("super::"):
            base_parts = base_parts[:-1]
            t = t[len("super::"):]
    elif "://" in t or t.startswith(("http://", "https://")):
        return ("", False)
    else:
        base_parts = cur_mid.split("::")[1:]
        if base_parts == [""]:
            base_parts = []
    rest = t.split("/")
    anchor = None
    if len(rest) > 1:
        t, anchor = "/".join(rest[:-1]), rest[-1]
    mid_parts = base_parts + ([x for x in t.split("::") if x] if t else [])
    # try longest matching module id
    for cut in range(len(mid_parts), 0, -1):
        cand = "vault::" + "::".join(mid_parts[:cut])
        if cand in modules:
            url = _rel_url(cur_dir, root / modules[cand])
            text = mid_parts[:cut][-1] if cut else "vault"
            link = f"[{text}]({url})"
            if anchor:
                link = f"[{'/'.join(mid_parts[cut:] + [anchor]) or text}]({url}#{slug(anchor)})"
            return (link, True)
    return ("", False)


def slug(s: str) -> str:
    return s.strip().lower().replace(" ", "-")


def _rel_url(cur_dir: Path, dest_file: Path) -> str:
    import os
    return os.path.relpath(dest_file, cur_dir)


def convert_text(src: str, cur_mid: str, modules: dict[str, Path], cur_dir: Path,
                 root: Path) -> tuple[str, int]:
    out_lines: list[str] = []
    unresolved = 0
    fence_open = False  # inside backtick-fence of any length
    open_fence_len = ""
    callout_buf: list[str] | None = None
    callout_depth = 0
    constructor_re = re.compile(r"^\s*#[a-zA-Z][a-zA-Z0-9_-]*\s*\(")
    # CommonMark-strict fences: opener = ticks + optional lang/info string, EOL.
    fence_open_re = re.compile(r"^\s*(`{3,})[ \t]*[A-Za-z0-9_+#.\-]*[ \t]*$")
    fence_close_re = re.compile(r"^\s*(`{3,})[ \t]*$")

    def flush_callout():
        nonlocal callout_buf, callout_depth
        if callout_buf is not None:
            out_lines.append("```not")
            out_lines.extend(callout_buf)
            out_lines.append("```")
            callout_buf = None
            callout_depth = 0

    for raw in src.splitlines():
        line = raw
        stripped = line.strip()

        fence_match = fence_open_re.match(line) if not fence_open else None
        close_match = fence_close_re.match(line) if fence_open else None
        if fence_open:
            out_lines.append(line)
            if close_match and len(close_match.group(1)) >= len(open_fence_len):
                fence_open = False
            continue
        if fence_match:
            flush_callout()
            fence_open = True
            open_fence_len = fence_match.group(1)
            out_lines.append(line)
            continue

        if callout_buf is not None:
            callout_buf.append(line)
            callout_depth += line.count("(") + line.count("[") - line.count(")") - line.count("]")
            if callout_depth <= 0 and (line.rstrip().endswith(")") or line.rstrip().endswith("]")):
                flush_callout()
            continue

        if BLOCK_ANN_RE.match(stripped) or MODULE_ANN_RE.match(stripped):
            continue

        if constructor_re.match(line):
            callout_buf = [line]
            callout_depth = line.count("(") + line.count("[") - line.count(")") - line.count("]")
            if callout_depth <= 0:
                flush_callout()
            continue

        # strip annotation postfixes but keep content
        line = ANN_POSTFIX_RE.sub("]", line)

        def ref_sub(m: re.Match) -> str:
            nonlocal unresolved
            url, ok = resolve_ref(m.group(1), cur_mid, modules, cur_dir, root)
            if not ok:
                unresolved += 1
                return f"`<{m.group(1)}>`"
            return url

        line = REF_RE.sub(ref_sub, line)
        out_lines.append(line)

    flush_callout()
    return "\n".join(out_lines) + ("\n" if src.endswith("\n") else ""), unresolved


def main() -> None:
    src_root = Path(sys.argv[1]).resolve()
    dst_root = Path(sys.argv[2]).resolve()
    if dst_root.exists():
        shutil.rmtree(dst_root)
    dst_root.mkdir(parents=True)

    files = sorted(p for p in src_root.rglob("*.not")
                   if not any(part in {"ai", ".obsidian"} for part in p.parts))
    modules: dict[str, Path] = {}
    for p in files:
        rel = p.relative_to(src_root)
        modules[module_id(rel)] = rel.with_suffix(".md")

    total_unresolved = 0
    for p in files:
        rel = p.relative_to(src_root)
        dest = dst_root / rel.with_suffix(".md")
        dest.parent.mkdir(parents=True, exist_ok=True)
        cur_dir = dest.parent
        src = p.read_text(encoding="utf-8")
        md, n_unres = convert_text(src, module_id(rel), modules, cur_dir, dst_root)
        total_unresolved += n_unres
        dest.write_text(md, encoding="utf-8")

    print(f"converted {len(files)} files; unresolved refs rendered literally: {total_unresolved}")


if __name__ == "__main__":
    main()
