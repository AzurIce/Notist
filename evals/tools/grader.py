#!/usr/bin/env python3
"""Mechanical grader for eval task outputs. Deterministic, no LLM."""
import json
import re
import sys
from pathlib import Path


def normalize(text: str) -> str:
    return text.strip()


def grade_task(task: dict, answer: str) -> dict:
    results = []
    ok_all = True
    for check in task.get("checks", []):
        ctype = check["type"]
        if ctype == "pattern_any":
            hit = any(re.search(p, answer) for p in check["patterns"])
            passed = hit
        elif ctype == "pattern_all":
            passed = all(re.search(p, answer) for p in check["patterns"])
        elif ctype == "stems_at_least":
            found = {s for s in check["stems"] if re.search(rf"\b{re.escape(s)}\b", answer)}
            passed = len(found) >= check["threshold"]
        else:
            raise ValueError(f"unknown check type {ctype}")
        results.append({"desc": check.get("desc", ctype), "type": ctype,
                        "passed": bool(passed)})
        ok_all = ok_all and bool(passed)
    return {"task": task["id"], "passed": ok_all, "checks": results}


def main() -> None:
    tasks_path = Path(sys.argv[1])
    out_dir = Path(sys.argv[2])  # contains <task>/answer.txt
    tasks = json.loads(tasks_path.read_text())["pilot"]["tasks"]
    by_id = {}
    for run_dir in sorted(p for p in out_dir.iterdir() if p.is_dir()):
        ans_file = run_dir / "answer.txt"
        tid = run_dir.name.split("__", 1)[0]
        if not ans_file.exists():
            continue
        task = next((t for t in tasks if t["id"] == tid), None)
        if task is None:
            print(f"no task def for {run_dir.name}", file=sys.stderr)
            continue
        g = grade_task(task, normalize(ans_file.read_text()))
        g["run"] = run_dir.name
        by_id[run_dir.name] = g
    report = {"runs": by_id,
              "passed": sum(1 for g in by_id.values() if g["passed"]),
              "total": len(by_id)}
    (out_dir / "grades.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(json.dumps(report["runs"], ensure_ascii=False, indent=1))
    print(f"passed {report['passed']}/{report['total']}")


if __name__ == "__main__":
    main()
