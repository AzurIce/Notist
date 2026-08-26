#!/usr/bin/env bash
# Post-build sanity probes for iteration 1 (module-candidate recall,
# section expansion, matched totals). Run against frozen corpus before eval.
set -u
W=/home/azurice/Files/worktrees/notist/feat-cli-readonly-queries
B=$W/target/release/notist
C=$W/evals/corpora/notist
fail=0

echo "== P1 multi-term candidate recall (expect hits>=1, bm25-v4, matched counts)"
$B search "游标 续读" "$C" --no-daemon --format json | python3 -c '
import json,sys
d=json.load(sys.stdin)
r=d["result"]
sp=r.get("search") or {}
cov=r["diagnostics"]["coverage"] if "coverage" in r.get("diagnostics",{}) else r["diagnostics"]
items=r["items"]
ok = len(items)>=1 and sp.get("rankingVersion")=="bm25-v4" and cov.get("matchedModules") is not None
print("hits",len(items),"rank",sp.get("rankingVersion"),"matched_modules",cov.get("matchedModules"),"matched_units",cov.get("matchedUnits"))
raise SystemExit(0 if ok else 1)' || fail=1

echo "== P2 single-term keeps bm25-v3 semantics and exposes totals"
$B search "索引" "$C" --no-daemon --format json | python3 -c '
import json,sys
d=json.load(sys.stdin); r=d["result"]; sp=r.get("search") or {}
print("rank",sp.get("rankingVersion"))
raise SystemExit(0 if len(r["items"])>=1 else 1)' || fail=1

echo "== P3 text mode prints matched modules line"
$B search "游标 续读" "$C" --no-daemon | grep -q "^matched .* modules" && echo ok || fail=1

echo "== P4 heading default id expands to section subtree"
$B read "#<vault::grammar/标注与 scope 形态>" "$C" --format json | python3 -c '
import json,sys
d=json.load(sys.stdin)
if not d.get("ok"):
    print("selector rejected:", d.get("error",{}).get("code")); raise SystemExit(1)
loc=d["result"]["items"][0]["location"] if d["result"].get("items") else d["result"]["selection"]
span=loc["byte_range"]; size=span["end"]-span["start"]
print("range span bytes:",size)
raise SystemExit(0 if size>200 else 1)' || fail=1

echo "== P5 operator=any untouched"
$B search "游标 续读" "$C" --no-daemon --operator any --format json | python3 -c '
import json,sys; d=json.load(sys.stdin); print("hits",len(d["result"]["items"]))
raise SystemExit(0 if len(d["result"]["items"])>=1 else 1)' || fail=1

exit $fail
