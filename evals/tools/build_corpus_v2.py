#!/usr/bin/env python3
"""Corpus v2 builder: enhance + convert, deterministic, both variants.

Usage:
  build_corpus_v2.py prepare <src_docs_root> <out_notist_root>
      extract official+dated docs, add derived module attributes, empty manifest
  build_corpus_v2.py convert <notist_root> <out_markdown_root>
      strict-Markdown transformation (v2 grammar)

Strictness targets (see eval-framework spec): real ATX headings, **strong**,
ordered digit lists, frontmatter from module attributes, stable HTML anchors,
inline/backtick-safe transforms, fenced fallback for bare constructors.
"""
import re
import shutil
import subprocess
import sys
from pathlib import Path

FENCE_OPEN = re.compile(r"^(\s*)(`{3,})[ \t]*[A-Za-z0-9_+#.\-]*[ \t]*$")
FENCE_CLOSE = re.compile(r"^(\s*)(`{3,})[ \t]*$")
HEADING = re.compile(r"^(={1,6})[ \t]+(.+?)\s*$")
ORDERED_ITEM = re.compile(r"^(\s*)\+[ \t]+(.+)$")
ANN_TAIL = re.compile(r"\]@([A-Za-z0-9_.,=\"'()/:\u4e00-\u9fff -]+)(?=\s|$)")
MODULE_PROPS = re.compile(r"^@!\[(.*)\]\s*$")
BLOCK_ANN = re.compile(r"^@\[(.*)\]\s*$")
REF = re.compile(r"#<([A-Za-z0-9_./:\-\u4e00-\u9fff]+)>")
CTOR_CALL = re.compile(r"^\s*#[a-zA-Z][a-zA-Z0-9_-]*\s*\(")

EXCLUDE_DIRS = {"ai"}  # v1 exclusion set; v2 keeps everything, placeholder unused


# ---------------------------------------------------------------- helpers

def git_archive(repo: Path, commit: str, sub: str, dest: Path) -> None:
    dest.mkdir(parents=True, exist_ok=True)
    arc = subprocess.run(["git", "-C", str(repo), "archive", commit, "--", sub],
                         capture_output=True, check=True)
    tar = subprocess.run(["tar", "-x", "-C", str(dest)], input=arc.stdout, check=True)


def derive_module_id(rel: Path) -> str:
    parts = list(rel.with_suffix("").parts)
    if parts[-1] == "README":
        parts = parts[:-1] or ["vault"]
    else:
        parts = ["vault"] + parts
    return "::".join(parts)


def classify(rel: Path) -> tuple[str, str]:
    """Return (kind, status) module attribute values from the source layout."""
    p = rel.as_posix()
    if p.startswith("ai/"):
        return ("research", "archived")
    if p.startswith("designs/") or p == "designs":
        return ("design", "current")
    if p.startswith("obsidian-notist/"):
        return ("integration-doc", "current")
    return ("reference", "current")


def md_rel_for(mid: str) -> Path:
    parts = mid.split("::")[1:]
    if not parts or parts == [""]:
        return Path("README.md")
    if parts[-1] == "" or parts[-1] == "vault":
        pass
    target = Path(*parts).with_suffix(".md") if parts[:-1] or True else Path("README.md")
    return target


def rel_url(cur_dir: Path, root: Path, dest_file: Path) -> str:
    import os
    return os.path.relpath(root / dest_file, cur_dir).replace("\\", "/")


def gh_slug(text: str, taken: dict[str, int]) -> str:
    s = re.sub(r"[^\w\u4e00-\u9fff\- ]", "", text.strip().lower())
    s = re.sub(r"[ \t]+", "-", s)
    base = s or "section"
    n = taken.get(base, 0)
    taken[base] = n + 1
    return base if n == 0 else f"{base}-{n}"


INLINE_CODE_RUN = re.compile(r"`+")


def protect_inline_code(line: str):
    """Replace paired backtick runs with sentinels; return (line, stash)."""
    stash: list[str] = []
    out = []
    i = 0
    while i < len(line):
        m = INLINE_CODE_RUN.match(line, i)
        if m:
            run = m.group(0)
            close = line.find(run, i + len(run))
            if close != -1:
                stash.append(line[i:close + len(run)])
                out.append(f"\x00{len(stash)-1}\x00")
                i = close + len(run)
                continue
        out.append(line[i])
        i += 1
    return "".join(out), stash


def unprotect(line: str, stash) -> str:
    return re.sub(r"\x00(\d+)\x00", lambda m: stash[int(m.group(1))], line)


# ---------------------------------------------------------------- prepare

def cmd_prepare(src: Path, dst: Path) -> None:
    repo = Path("/home/azurice/Files/notist")
    commit = subprocess.run(["git", "-C", str(repo), "rev-parse", "HEAD"],
                            capture_output=True, text=True).stdout.strip()
    if dst.exists():
        shutil.rmtree(dst)
    dst.mkdir(parents=True)
    tmp = dst.parent / "_archive_tmp"
    shutil.rmtree(tmp, ignore_errors=True)
    tmp.mkdir()
    git_archive(repo, commit, "docs", tmp)
    shutil.copytree(tmp / "docs", dst, dirs_exist_ok=True)
    shutil.rmtree(tmp)
    shutil.rmtree(dst / ".obsidian", ignore_errors=True)
    (dst / "AGENTS.md").unlink(missing_ok=True)
    # bundle the plugin packages the docs call out so mermaid/shader register
    for name in ("shader", "mermaid"):
        src_pkg = repo / "plugins" / name
        if src_pkg.is_dir():
            shutil.copytree(src_pkg, dst / "plugins" / name, dirs_exist_ok=True)
    (dst / "Notist.toml").write_text(
        '[plugins.shader]\npath = "plugins/shader"\n\n'
        '[plugins.mermaid]\npath = "plugins/mermaid"\n')
    (dst / ".corpus-source-commit").write_text(commit + "\n")

    # derived module attributes for every source-backed module lacking them
    changed = 0
    for p in sorted(dst.rglob("*.not")):
        rel = p.relative_to(dst)
        kind, status = classify(rel)
        lines_all = p.read_text(encoding="utf-8").splitlines(keepends=True)
        head = "".join(lines_all[:4])
        existing = MODULE_PROPS.match(head.splitlines()[0].strip()) if head.startswith("@![") else None
        if existing:
            if "status" in existing.group(1):
                continue
            merged = existing.group(1) + f', kind = "{kind}", status = "{status}"'
            new_first = f"@![{merged}]\n"
            p.write_text(new_first + "".join(lines_all[1:]), encoding="utf-8")
        else:
            attrs = f'@![kind = "{kind}", status = "{status}"]'
            p.write_text(attrs + "\n" + "".join(lines_all), encoding="utf-8")
        changed += 1
    print(f"prepared {dst}; source_commit={commit[:12]}; attrs_added={changed}")


# ---------------------------------------------------------------- convert

class Converter:
    def __init__(self, src_root: Path, dst_root: Path):
        self.src = src_root
        self.dst = dst_root
        self.modules: dict[str, Path] = {}
        for p in sorted(src_root.rglob("*.not")):
            self.modules[derive_module_id(p.relative_to(src_root))] = \
                p.relative_to(src_root).with_suffix(".md")

    def slug_for(self, mid: str, title: str,
                 slugs_by_mod: dict[str, dict[str, str]]) -> str | None:
        return slugs_by_mod.get(mid, {}).get(title)

    def resolve_ref(self, target: str, cur_mid: str, cur_dir: Path,
                    slugs_by_mod: dict[str, dict[str, str]]):
        t = target
        if t.startswith("vault::"):
            base_parts, t = [], t[len("vault::"):]
        elif t.startswith("self::"):
            base_parts = [p for p in cur_mid.split("::")[1:] if p]
            t = t[len("self::"):]
        elif t.startswith("super::"):
            base_parts = [p for p in cur_mid.split("::")[1:-1] if p]
            while t.startswith("super::"):
                base_parts = base_parts[:-1]
                t = t[len("super::"):]
        elif "://" in t:
            return ""
        else:
            base_parts = [p for p in cur_mid.split("::")[1:] if p]
        segs = t.split("/")
        anchor = None
        if len(segs) > 1:
            t, anchor = "/".join(segs[:-1]), segs[-1]
        mid_parts = base_parts + ([x for x in t.split("::") if x] if t else [])
        for cut in range(len(mid_parts), 0, -1):
            cand = "vault::" + "::".join(mid_parts[:cut])
            if cand in self.modules:
                url = rel_url(cur_dir, self.dst, self.modules[cand])
                label_text = mid_parts[cut - 1]
                frag = ""
                rest = mid_parts[cut:]
                if rest:
                    frag = "#" + gh_slug("/".join(rest), {})
                elif anchor:
                    s = slugs_by_mod.get(cand, {}).get(anchor)
                    frag = "#" + (s if s else anchor.lower())
                linktext = "/".join(rest) if rest else label_text
                return f"[{linktext}]({url}{frag})"
        return ""

    def convert_file(self, rel: Path, slugs_by_mod: dict[str, dict[str, str]],
                     taken_global: dict[str, int]) -> tuple[str, dict]:
        src_path = self.src / rel
        cur_mid = derive_module_id(rel)
        cur_dir = self.dst / rel.parent
        meta = {"unresolved_refs": 0}
        raw_lines = src_path.read_text(encoding="utf-8").splitlines()

        # frontmatter for module attribute lines near the top
        fm_entries: list[tuple[str, str]] = []
        body_start = 0
        for idx, ln in enumerate(raw_lines[:4]):
            m = MODULE_PROPS.match(ln)
            if m:
                for piece in m.group(1).split(","):
                    if "=" in piece:
                        k, v = piece.split("=", 1)
                        fm_entries.append((k.strip(), v.strip().strip('"')))
                    elif piece.startswith("#"):
                        fm_entries.append(("tags", piece[1:]))
                    else:
                        fm_entries.append(("misc", piece))
                body_start = idx + 1
                break

        headings: list[tuple[int, str, str]] = []  # (line_idx, level, text)
        states = []  # 'code' | 'ctor' | 'normal'
        fence_open_len = 0
        ctor_buf_end = -1
        depth = 0
        ctor_start = None
        for i, ln in enumerate(raw_lines[body_start:], start=body_start):
            if fence_open_len:
                states.append("code")
                mc = FENCE_CLOSE.match(ln)
                if mc and len(mc.group(2)) >= fence_open_len:
                    fence_open_len = 0
                continue
            mo = FENCE_OPEN.match(ln)
            if mo:
                states.append("code")
                fence_open_len = len(mo.group(2))
                continue
            hm = HEADING.match(ln)
            if hm:
                states.append("heading")
                headings.append((i, len(hm.group(1)), hm.group(2)))
                continue
            if ctor_start is not None:
                states.append("ctor")
                depth += ln.count("(") + ln.count("[") - ln.count(")") - ln.count("]")
                s = ln.rstrip()
                if depth <= 0 and (s.endswith(")") or s.endswith("]")):
                    ctor_buf_end = i
                    ctor_start = None
                continue
            cm = CTOR_CALL.match(ln)
            if cm:
                states.append("ctor")
                ctor_start = i
                depth = ln.count("(") + ln.count("[") - ln.count(")") - ln.count("]")
                if depth <= 0:
                    ctor_buf_end = i
                    ctor_start = None
                continue
            if BLOCK_ANN.match(ln.strip()):
                states.append("blockann")
                continue
            states.append("normal")

        # gather heading titles -> slugs for this module
        local_slugs: dict[str, str] = {}
        taken: dict[str, int] = {}
        for _, _, title in headings:
            local_slugs[title] = gh_slug(title, taken)

        out: list[str] = []
        ctor_pending = False
        ordered_run_level: str | None = None
        ordered_n: dict[int, int] = {}
        hidx = 0
        unresolved = 0

        def flush_ctor(buf):
            out.append("```not")
            out.extend(buf)
            out.append("```")

        buf: list[str] = []
        for pos, (i, st) in enumerate(zip(range(body_start, len(raw_lines)), states)):
            ln = raw_lines[i]
            if st == "code":
                out.append(ln)
                continue
            if st == "ctor":
                buf.append(ln)
                if i == ctor_buf_end:
                    flush_ctor(buf)
                    buf = []
                continue
            if st == "blockann":
                pieces = BLOCK_ANN.match(ln.strip()).group(1)
                out.append(f"<!-- notist-block-annotation: {pieces} -->")
                continue
            if st == "heading":
                _, lvl, title = headings[hidx]
                hidx += 1
                slug = local_slugs[title]
                if lvl <= 3:
                    out.append(f'<a id="{slug}"></a>')
                out.append("#" * lvl + " " + title)
                continue
            # ---- normal line
            line = ln
            line, stash = protect_inline_code(line)

            om = ORDERED_ITEM.match(line)
            if om:
                indent = len(om.group(1))
                lvlkey = indent // 2
                if ordered_run_level is None or indent < len(ordered_run_level):
                    ordered_n = {}
                ordered_n[lvlkey] = ordered_n.get(lvlkey, 0) + 1
                num = ".".join(str(ordered_n[k]) for k in sorted(ordered_n) if k <= lvlkey)
                line = f"{om.group(1)}{num}. {om.group(2)}"
                ordered_run_level = " " * indent
            elif line.strip():
                ordered_run_level = None

            def ann_tail_sub(m):
                return "] <!-- @ann: " + m.group(1).strip() + " -->"

            line = ANN_TAIL.sub(ann_tail_sub, line)

            def ref_sub(m):
                nonlocal unresolved
                rep = self.resolve_ref(m.group(1), cur_mid, cur_dir, slugs_by_mod)
                if not rep:
                    unresolved += 1
                    return "`<" + m.group(1) + ">`"
                return rep

            line = REF.sub(ref_sub, line)

            # notist *strong* -> markdown **strong** (conservative boundaries)
            line = re.sub(r"(?<![*\w\\])\*(\S(?:[^*\n]*?\S)?)\*(?![*\w])",
                          r"**\1**", line)
            # notist __underline__ -> ***bold italic***
            line = re.sub(r"(?<![_\w\\])__(\S(?:[^_\n]*?\S)?)__(?![_\w])",
                          r"***\1***", line)

            line = unprotect(line, stash)
            if line.rstrip() == "" and False:
                pass
            out.append(line)

        header = ""
        if fm_entries:
            keys: list[str] = []
            for k, v in fm_entries:
                if k not in keys:
                    keys.append(k)
            seen_rows = {k: [] for k in keys}
            for k, v in fm_entries:
                seen_rows.setdefault(k, []).append(v)
            header_lines = ["---"]
            for k in keys:
                vals = seen_rows[k]
                header_lines.append(f"{k}: {'[' + ', '.join(vals) + ']' if len(vals) > 1 else vals[0]}")
            header_lines.append("---\n")
            header = "\n".join(header_lines)

        doc = "\n".join(([header] if header else []) + out) + "\n"
        meta["unresolved_refs"] = unresolved
        return doc, meta


def cmd_convert(src: Path, dst: Path) -> None:
    if dst.exists():
        shutil.rmtree(dst)
    dst.mkdir(parents=True)
    conv = Converter(src, dst)
    totals = 0
    for p in sorted(src.rglob("*.not")):
        rel = p.relative_to(src)
        slugs_by_mod: dict[str, dict[str, str]] = {}
        # pre-compute slug maps module-wide (single pass cheap enough twice)
        slugs_by_mod.update(_slug_maps(src, conv.modules))
        doc, meta = conv.convert_file(rel, slugs_by_mod, {})
        dest = dst / rel.with_suffix(".md")
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(doc, encoding="utf-8")
        totals += meta["unresolved_refs"]
    print(f"converted {sum(1 for _ in src.rglob('*.not'))} files; "
          f"literal-rendered refs: {totals}")


_slug_cache = {}


def _slug_maps(src: Path, modules: dict[str, Path]):
    if not _slug_cache:
        for mid in modules:
            taken: dict[str, int] = {}
            p = src / modules[mid].with_suffix(".not")
            text = p.read_text(encoding="utf-8").splitlines()
            fmap = 0
            for ln in text[:4]:
                if MODULE_PROPS.match(ln):
                    fmap += 1
                    break
            for ln in text[fmap:]:
                hm = HEADING.match(ln)
                if hm:
                    local = gh_slug(hm.group(2), taken)
            _slug_cache[mid] = _mod_titles(src, mid, modules)
    return _slug_cache


def _mod_titles(src: Path, mid: str, modules: dict[str, Path]) -> dict[str, str]:
    p = src / modules[mid].with_suffix(".not")
    if not p.exists():
        return {}
    lines = p.read_text(encoding="utf-8").splitlines()
    fmap = 0
    for ln in lines[:4]:
        if MODULE_PROPS.match(ln):
            fmap += 1
            break
    taken: dict[str, int] = {}
    out: dict[str, str] = {}
    fence = False
    flen = 0
    for ln in lines[fmap:]:
        mo = FENCE_OPEN.match(ln)
        mc = FENCE_CLOSE.match(ln)
        if fence:
            if mc and len(mc.group(2)) >= flen:
                fence = False
            continue
        if mo:
            fence = True
            flen = len(mo.group(2))
            continue
        hm = HEADING.match(ln)
        if hm:
            out[hm.group(2)] = gh_slug(hm.group(2), taken)
    return out


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "prepare":
        cmd_prepare(Path(sys.argv[2]).resolve(), Path(sys.argv[3]).resolve())
    elif cmd == "convert":
        cmd_convert(Path(sys.argv[2]).resolve(), Path(sys.argv[3]).resolve())
    else:
        sys.exit("unknown command")
