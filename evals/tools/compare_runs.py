#!/usr/bin/env python3
"""Compare an iteration run against the baseline run.

Usage: compare_runs.py <baseline_run_dir> <iteration_run_dir>
Prints a pass matrix delta and token/time totals parsed from archived
session JSONLs.
"""
import json
import sys
from pathlib import Path


def load(run_dir: Path, budgets: dict | None = None,
         factors: dict | None = None) -> dict:
    out = {}
    grades = json.loads((run_dir / "output" / "grades.json").read_text())
    for d in sorted((run_dir / "output").iterdir()):
        if not d.is_dir():
            continue
        meta = json.loads((d / "meta.json").read_text())
        grades = json.loads((run_dir / "output" / "grades.json").read_text())
        g = grades["runs"].get(d.name, {})
        toks = int(meta.get("tokens_total") or 0)
        violation = (d / "AUDIT_VIOLATION").exists()
        cond, task = meta["condition"], meta["task"]
        base_budget = (budgets or {}).get(task)
        factor = (factors or {}).get(cond, 1.0)
        budget = base_budget * factor if base_budget else None
        over_budget = bool(budget and toks > budget)
        passed = g.get("passed", False) and not violation and not over_budget
        out.setdefault(cond, {})[task] = {
            "passed": passed,
            "reason": "" if g.get("passed") else "wrong"
                      + ("+violation" if violation else "")
                      + ("+over_budget" if over_budget else ""),
            "duration": meta.get("duration_s"),
            "tokens": toks,
            "rc": meta.get("returncode"),
        }
    return out


TASKS_FILE = Path(__file__).resolve().parent.parent / "tasks.json"


def main() -> None:
    spec = json.loads(TASKS_FILE.read_text())["pilot"]
    tasks_spec = spec["tasks"]
    budgets = {t["id"]: t.get("budget_tokens") for t in tasks_spec}
    factors = spec.get("budget_condition_factors", {})
    base = load(Path(sys.argv[1]), budgets, factors)
    it = load(Path(sys.argv[2]), budgets, factors)
    conds = sorted({c for side in (base, it) for c in side})
    tasks = sorted({t for side in (base, it) for c in side.values() for t in c})
    print(f"{'cond/task':38s} {'base':>5s} -> {'iter':>5s}   dur(b->i,s)   tok(b->i)")
    flips = {"fail->pass": [], "pass->fail": []}
    tb = ti = pb = pi = 0
    for cond in conds:
        for t in tasks:
            b = base.get(cond, {}).get(t)
            i = it.get(cond, {}).get(t)
            if not b and not i:
                continue
            bp, ip = bool(b and b["passed"]), bool(i and i["passed"])
            pb += bp; pi += ip; tb += b["tokens"] if b else 0; ti += i["tokens"] if i else 0
            mark = ""
            if b and i:
                if not bp and ip: mark = "  << FIXED"; flips["fail->pass"].append(f"{cond}/{t}")
                elif bp and not ip: mark = "  !! REgressED"; flips["pass->fail"].append(f"{cond}/{t}")
            db = f"{b['duration']:.0f}->{i['duration']:.0f}" if b and i else "-"
            tk = f"{b['tokens']}->{i['tokens']}" if b and i else "-"
            print(f"{cond+'/'+t.split('-')[0]:38s} {str(bp):>5s} -> {str(ip):>5s}   {db:>12s}   {tk:>16s}{mark}")
    print(f"\ntotal passed: {pb} -> {pi} / {len([1 for c in conds for t in tasks])}")
    print(f"total tokens: {tb} -> {ti}")
    print("fixed:", flips["fail->pass"] or "-")
    print("regressed:", flips["pass->fail"] or "-")


if __name__ == "__main__":
    main()
