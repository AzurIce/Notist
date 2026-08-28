#!/usr/bin/env python3
"""Aggregate N eval runs into per-cell statistics (n>=3 protocol).

Usage: stats.py <run_dir> [run_dir ...]

For every (condition, task) cell present in the runs: pass rate, median and
min/max tokens, median duration. Prints the enforced/TC1 focus block and a
per-condition effective-pass line using tiered budgets from tasks.json.
"""
import json
import statistics
import sys
from pathlib import Path

EVALS = Path(__file__).resolve().parent.parent
spec = json.loads((EVALS / "tasks.json").read_text())["pilot"]
BUDGETS = {t["id"]: t.get("budget_tokens") for t in spec["tasks"]}
FACTORS = spec.get("budget_condition_factors", {})


def collect(run_dir: Path):
    out = {}
    grades = json.loads((run_dir / "output" / "grades.json").read_text())
    for d in sorted((run_dir / "output").iterdir()):
        if not d.is_dir():
            continue
        meta = json.loads((d / "meta.json").read_text())
        g = grades["runs"].get(d.name, {})
        violation = (d / "AUDIT_VIOLATION").exists()
        cond, task = meta["condition"], meta["task"]
        toks = int(meta.get("tokens_total") or 0)
        budget = BUDGETS.get(task)
        budget *= FACTORS.get(cond, 1.0)
        over = bool(budget and toks > budget)
        passed = bool(g.get("passed")) and not violation and not over
        out.setdefault((cond, task), []).append({
            "passed": passed, "tokens": toks,
            "duration": meta.get("duration_s") or 0,
            "timeout": bool(meta.get("timed_out")),
            "violation": violation,
        })
    return out


def main() -> None:
    merged = {}
    names = []
    for arg in sys.argv[1:]:
        names.append(Path(arg).name)
        for key, runs in collect(Path(arg)).items():
            merged.setdefault(key, []).extend(runs)

    focus = merged.get(("notist-enforced", "TC1-vector-retrieval-absence"), [])
    print("== FOCUS enforced/TC1")
    if focus:
        toks = sorted(r["tokens"] for r in focus)
        print(f" n={len(focus)} tokens={toks} median={statistics.median(toks):.0f} "
              f"pass={sum(r['passed'] for r in focus)}/{len(focus)} "
              f"timeouts={sum(r['timeout'] for r in focus)}")
        over = sum(1 for t in toks if t > 26000 * FACTORS.get("notist-enforced", 1.0))
        print(f" over tiered cap(52k): {over}/{len(toks)}")

    conds = sorted({c for c, _ in merged})
    tasks = sorted({t for _, t in merged})
    print("\n== cells (n>=2 shown with median/range)")
    for cond in conds:
        for task in tasks:
            runs = merged.get((cond, task))
            if not runs:
                continue
            toks = [r["tokens"] for r in runs]
            line = (f"{cond:16s} {task[:34]:34s} pass {sum(r['passed'] for r in runs)}/{len(runs)}"
                    f" tok med={statistics.median(toks):>7.0f} range={min(toks)}-{max(toks)}")
            if len(runs) >= 2:
                line += f" cv={(statistics.pstdev(toks)/max(statistics.mean(toks),1)):.2f}"
            print(line)

    print("\n== effective pass per condition")
    for cond in conds:
        cells = [(c, t) for (c, t) in merged if c == cond]
        p = sum(sum(r["passed"] for r in merged[k]) for k in cells)
        n = sum(len(merged[k]) for k in cells)
        print(f"{cond:16s} {p}/{n}")


if __name__ == "__main__":
    main()
