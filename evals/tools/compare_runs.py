#!/usr/bin/env python3
"""Compare an iteration run against the baseline run.

Usage: compare_runs.py <baseline_run_dir> <iteration_run_dir>
Prints a pass matrix delta and token/time totals parsed from archived
session JSONLs.
"""
import json
import sys
from pathlib import Path


def load(run_dir: Path) -> dict:
    out = {}
    for d in sorted((run_dir / "output").iterdir()):
        if not d.is_dir():
            continue
        meta = json.loads((d / "meta.json").read_text())
        answer = (d / "answer.txt").read_text() if (d / "answer.txt").exists() else ""
        grades = json.loads((run_dir / "output" / "grades.json").read_text())
        g = grades["runs"].get(d.name, {})
        toks = 0
        sess_dir = d / "session"
        if sess_dir.is_dir():
            for f in sess_dir.glob("*.jsonl"):
                for line in f.read_text().splitlines():
                    try:
                        rec = json.loads(line)
                    except json.JSONDecodeError:
                        continue
                    msg = rec.get("message") or {}
                    usage = msg.get("usage") or {}
                    toks += (usage.get("input") or 0) + (usage.get("output") or 0)
        cond, task = meta["condition"], meta["task"]
        out.setdefault(cond, {})[task] = {
            "passed": g.get("passed", False),
            "duration": meta.get("duration_s"),
            "tokens": toks,
            "rc": meta.get("returncode"),
        }
    return out


def main() -> None:
    base = load(Path(sys.argv[1]))
    it = load(Path(sys.argv[2]))
    conds = ["null-notools", "md-bash", "notist-bash", "notist-skill"]
    tasks = ["T1-locate-grammar-spec", "T2-callout-signature",
             "T3-pipeline-inventory", "T4-module-annotation-syntax",
             "T5-table-constructor-section-read", "T6-cursor-continuation-rule-doc"]
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
