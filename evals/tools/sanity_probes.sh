#!/usr/bin/env bash
# Post-build sanity probes for read-only CLI iterations.
# iteration-1 expectations: module-candidate recall (bm25-v4), coverage
# totals in JSON (snake_case fields), matched line in text mode,
# heading-default-id selectors expand to whole sections.
set -u
W=/home/azurice/Files/worktrees/notist/feat-cli-readonly-queries
B=$W/target/release/notist
C=$W/evals/corpora/notist
fail=0

echo "P1 multi-term candidate recall"
$B search "cursor 续读" "$C" --no-daemon --format json | python3 -c '
import json,sys
r=json.load(sys.stdin)["result"]
sp=r["search"]; cov=r["coverage"]
ok = len(r["items"])>=1 and sp.get("ranking_version")=="bm25-v4" \
     and (cov.get("matched_modules") or 0)>=3
print(" hits",len(r["items"]),"mm",cov.get("matched_modules"),"mu",cov.get("matched_units"))
raise SystemExit(0 if ok else 1)' || fail=1

echo "P2 single-term stays bm25-v3 and exposes totals"
$B search "续读" "$C" --no-daemon --format json | python3 -c '
import json,sys
r=json.load(sys.stdin)["result"]
ok = r["search"].get("ranking_version")=="bm25-v3" and r["coverage"].get("matched_units") is not None
print(" rank",r["search"].get("ranking_version"),"mu",r["coverage"].get("matched_units"))
raise SystemExit(0 if ok else 1)' || fail=1

echo "P3 text mode prints matched line"
$B search "cursor 续读" "$C" --no-daemon | grep -q "^matched .* modules" && echo ok || fail=1

echo "P4 heading id expands to section subtree (expect span >200 bytes)"
$B read "grammar.not#标注与 scope 形态" "$C" --format json | python3 -c '
import json,sys
d=json.load(sys.stdin)
br=d["result"]["items"][0]["location"]["byte_range"]
size=br["end"]-br["start"]
print(" span",size,"bytes, end at line-anchor of next heading")
raise SystemExit(0 if size>200 else 1)' || fail=1

echo "P5 zero-hit negative still complete+hinted"
$B search "不存在的词组zzz" "$C" --no-daemon --format json | python3 -c '
import json,sys
r=json.load(sys.stdin)["result"]
cov=r["coverage"]
ok = len(r["items"])==0 and cov["complete"] and cov.get("matched_modules")==0
raise SystemExit(0 if ok else 1)' && echo ok || fail=1


echo "P6 search hits carry section attribution"
$B search "续读" "$C" --no-daemon --format json | python3 -c '
import json,sys
items=json.load(sys.stdin)["result"]["items"]
hits=[i for i in items if i.get("section_title") or i.get("section_id")]
print(" with-section:",len(hits),"/",len(items))
raise SystemExit(0 if hits else 1)' || fail=1
echo "P7 exclude-scope drops archive and updates totals"
ALL=$($B search "检索" "$C" --no-daemon --format json | python3 -c 'import json,sys;r=json.load(sys.stdin)["result"];print(r["coverage"]["matched_modules"])')
EX=$($B search "检索" "$C" --no-daemon --format json --exclude-scope "vault::ai" | python3 -c '
import json,sys
r=json.load(sys.stdin)["result"]; cov=r["coverage"]
mods={i["location"]["module"] for i in r["items"]}
assert not any(m.startswith("vault::ai") for m in mods), "ai leaked under exclusion"
print(cov["matched_modules"])')
python3 -c "import sys;a,b=int('$ALL'),int('${EX:-0}');sys.exit(0 if 0<b<a else 1)" \
  && echo " ok: $ALL -> $EX after excluding vault::ai" || { echo FAIL; exit 1; }
echo "P8 multi-term query exposes per-scope module buckets"
$B search "向量 嵌入" "$C" --no-daemon --format json | python3 -c '
import json,sys
cov=json.load(sys.stdin)["result"]["coverage"]
bd=cov.get("scopes_breakdown") or {}
ok = bd.get("ai",0)>=1 and bd.get("designs",0)>=1
print(" breakdown:",bd)
raise SystemExit(0 if ok else 1)' || fail=1
echo "P9 first-class absence verdict on both polarities"
$B search "不存在的词组zzz" "$C" --no-daemon --format json | python3 -c '
import json,sys
cov=json.load(sys.stdin)["result"]["coverage"]
ok = cov.get("conclusion")=="absent-in-snapshot"
print(" conclusion:",cov.get("conclusion"))
raise SystemExit(0 if ok else 1)' || fail=1
$B search "检索" "$C" --no-daemon --format json | python3 -c '
import json,sys
cov=json.load(sys.stdin)["result"]["coverage"]
ok = cov.get("conclusion")=="present"
print(" conclusion:",cov.get("conclusion"))
raise SystemExit(0 if ok else 1)' || fail=1
$B search "不存在的词组zzz" "$C" --no-daemon | grep -q "verdict: ABSENT" && echo " text verdict ok" || fail=1
exit $fail
