#!/usr/bin/env python3
"""Eval runner v2: hardened pi headless agents x 4 conditions x tasks.

Isolation audit conclusions (2026-08-27):
- every invocation passes --offline --no-context-files --no-extensions
  --no-skills --no-prompt-templates --no-themes so neither the ancestor
  AGENTS.md of the worktree nor global herdr extensions/skills/themes can leak;
- system prompt fully replaced by --system-prompt (verified by probe: only our
  text plus a cwd line reaches the model);
- tools allowlisted to read,bash,find,grep,ls for tool conditions; the
  null-tools condition gets --no-tools to measure parametric knowledge /
  hallucination rate;
- thinking pinned (--thinking high) because settings.json sets a default;
- session logs land directly under each output dir via --session-dir;
- sandboxes live outside the repo tree (/tmp) to dodge ancestor config pickup;
- PATH carries the worktree release binary; its resolved path and sha256 are
  recorded in the manifest.
"""
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

EVALS = Path(__file__).resolve().parent.parent
WORKTREE = EVALS.parent
NOTIST_BIN = WORKTREE / "target" / "release" / "notist"
MODEL = "lmstudio/qwen3.8-27b-uncensored-mlx"
PROVIDER = "lmstudio"
SKILL_MD = WORKTREE / "skills" / "notist" / "SKILL.md"

BASE_SYSTEM_PROMPT = (
    "你是文档检索助手，在只读任务中回答问题。\n"
    "你可以使用 bash、read、find、grep、ls 工具检查当前目录中的文档库。\n"
    "回答用中文，简明扼要：直接给出结论与出处（文件、节标题）。\n"
    "不要修改任何文件。"
)
NO_TOOL_SYSTEM_PROMPT = (
    "你是文档检索助手，直接凭你已有的知识回答问题。\n"
    "你没有可用工具。回答用中文，简明扼要：直接给出结论与出处（文件、节标题）。"
)
NOTIST_HINT = "\n环境提示：PATH 上有一个名为 notist 的本地 CLI 可用于查询这份文档库。"

ISOLATION_FLAGS = [
    "--offline", "--no-context-files", "--no-extensions",
    "--no-skills", "--no-prompt-templates", "--no-themes",
]
READONLY_TOOLS = ["read", "bash", "find", "grep", "ls"]

CONDITIONS = {
    # null baseline: no tools at all -> parametric knowledge / hallucination rate
    "null-notools": {
        "corpus": None,
        "system_prompt": NO_TOOL_SYSTEM_PROMPT,
        "skill": False,
        "tools": None,
    },
    "md-bash": {
        "corpus": "markdown",
        "system_prompt": BASE_SYSTEM_PROMPT,
        "skill": False,
        "tools": READONLY_TOOLS,
    },
    "notist-bash": {
        "corpus": "notist",
        "system_prompt": BASE_SYSTEM_PROMPT + NOTIST_HINT,
        "skill": False,
        "tools": READONLY_TOOLS,
    },
    "notist-skill": {
        "corpus": "notist",
        "system_prompt": BASE_SYSTEM_PROMPT + NOTIST_HINT,
        "skill": True,
        "tools": READONLY_TOOLS,
    },
}


def sha256_file(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()[:16]


def corpus_digest(root: Path) -> str:
    h = hashlib.sha256()
    for f in sorted(root.rglob("*")):
        if f.is_file():
            h.update(str(f.relative_to(root)).encode())
            h.update(f.read_bytes())
    return h.hexdigest()[:16]


def rebuild_sandbox(run_name: str, cond: str, corpus_kind: str | None) -> Path | None:
    # Neutral path under /tmp: avoids ancestor AGENTS.md pickup and keeps the
    # string "notist" out of md-bash agents' view of their own cwd.
    base = Path(tempfile.gettempdir()) / f"kb-eval-{run_name}" / cond
    if base.exists():
        shutil.rmtree(base)
    if corpus_kind is None:
        base.mkdir(parents=True)
        return base
    dest = base / "kb"
    shutil.copytree(EVALS / "corpora" / corpus_kind, dest)
    return dest


def run_one(cond: str, task: dict, runs_root: Path, timeout_s: int,
            args_resume: bool = False) -> dict:
    cfg = CONDITIONS[cond]
    out_dir = runs_root / "output" / f"{task['id']}__{cond}"
    if args_resume and (out_dir / "meta.json").exists():
        print(f"[{cond}] {task['id']}: skip (exists)", flush=True)
        return {"condition": cond, "task": task["id"], "returncode": "cached",
                "duration_s": None}
    out_dir.mkdir(parents=True, exist_ok=True)

    sandbox = rebuild_sandbox(runs_root.name, cond, cfg["corpus"])

    cmd = ["pi", "-p", "--provider", PROVIDER, "--model", MODEL,
           "--thinking", "high"] + ISOLATION_FLAGS
    cmd += ["--system-prompt", cfg["system_prompt"]]
    if cfg["skill"]:
        cmd += ["--skill", str(SKILL_MD)]
    if cfg["tools"]:
        cmd += ["--tools", ",".join(cfg["tools"])]
    else:
        cmd += ["--no-tools"]
    cmd += ["--session-dir", str(out_dir / "session")]
    cmd.append(task["prompt"])

    env = os.environ.copy()
    provenance = None
    if cond != "null-notools":
        assert NOTIST_BIN.exists(), NOTIST_BIN
        env["PATH"] = os.pathsep.join([str(NOTIST_BIN.parent), env.get("PATH", "")])
        resolved = shutil.which("notist", path=env["PATH"])
        if Path(resolved).resolve() != NOTIST_BIN.resolve():
            raise RuntimeError(f"PATH resolves notist to {resolved}, expected {NOTIST_BIN}")
        provenance = {"resolved": str(resolved), "sha256_16": sha256_file(NOTIST_BIN)}

    meta = {"condition": cond, "task": task["id"], "cmd": cmd,
            "cwd": str(sandbox), "notist_provenance": provenance,
            "started_utc": datetime.now(timezone.utc).isoformat()}
    start = time.time()
    def _text(v):
        if v is None:
            return ""
        if isinstance(v, bytes):
            return v.decode("utf-8", errors="replace")
        return v

    try:
        proc = subprocess.run(cmd, cwd=str(sandbox), env=env,
                              capture_output=True, text=True, timeout=timeout_s)
        meta["returncode"] = proc.returncode
        answer = _text(proc.stdout)
        meta["stderr_tail"] = _text(proc.stderr)[-1500:]
    except subprocess.TimeoutExpired as e:
        meta.update(returncode=None, timed_out=True)
        answer = _text(e.stdout)
        meta["stderr_tail"] = _text(e.stderr)[-1500:]
    meta["duration_s"] = round(time.time() - start, 1)

    (out_dir / "answer.txt").write_text(answer)
    sessions = sorted((out_dir / "session").glob("*.jsonl")) if (out_dir / "session").exists() else []
    meta["session_files"] = [str(p.relative_to(out_dir)) for p in sessions]
    (out_dir / "meta.json").write_text(json.dumps(meta, ensure_ascii=False, indent=2))
    print(f"[{cond}] {task['id']}: rc={meta.get('returncode')} "
          f"{meta['duration_s']}s sessions={len(sessions)}", flush=True)
    return {k: meta[k] for k in ("condition", "task", "returncode", "duration_s")}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--tasks", default=str(EVALS / "tasks.json"))
    ap.add_argument("--conditions", nargs="+", default=list(CONDITIONS))
    ap.add_argument("--only", nargs="*", help="task id prefixes")
    ap.add_argument("--timeout", type=int, default=600)
    ap.add_argument("--name", required=True)
    ap.add_argument("--skip-existing", action="store_true")
    args = ap.parse_args()

    spec = json.loads(Path(args.tasks).read_text())
    tasks = spec["pilot"]["tasks"]
    if args.only:
        tasks = [t for t in tasks if any(t["id"].startswith(o) for o in args.only)]

    runs_root = EVALS / "runs" / args.name
    runs_root.mkdir(parents=True)

    manifest = {
        "run": args.name,
        "model": MODEL,
        "commit": subprocess.run(["git", "-C", str(WORKTREE), "rev-parse", "HEAD"],
                                 capture_output=True, text=True).stdout.strip(),
        "corpora": {k: corpus_digest(EVALS / "corpora" / k)
                    for k in ("notist", "markdown")},
        "notist_sha256_16": sha256_file(NOTIST_BIN),
        "skill_md_sha256_16": sha256_file(SKILL_MD),
        "invocations": [],
    }
    for cond in args.conditions:
        for task in tasks:
            manifest["invocations"].append(
                run_one(cond, task, runs_root, args.timeout,
                    args.skip_existing))
            (runs_root / "manifest.json").write_text(
                json.dumps(manifest, ensure_ascii=False, indent=2))

    print(f"\nrun dir: {runs_root}", flush=True)
    subprocess.run([sys.executable, str(EVALS / "tools" / "grader.py"),
                    args.tasks, str(runs_root / "output")])


if __name__ == "__main__":
    main()
