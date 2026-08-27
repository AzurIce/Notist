#!/usr/bin/env python3
"""Build a zero-dependency static dashboard over evals/runs/*.

Outputs:
  evals/dashboard/index.html            self-contained UI (vanilla JS/CSS)
  evals/dashboard/data.js               window.EVAL_DASH = {runs:{...}}  (no sessions)
  evals/dashboard/sessions/<run>/<cell>.js   JSONP: window.EVAL_SESSIONS[key]=... (lazy)

Works from file:// (script-tag JSONP, no fetch).
"""
import html
import json
import shutil
from datetime import datetime
from pathlib import Path

EVALS = Path(__file__).resolve().parent.parent
RUNS = EVALS / "runs"
OUT = EVALS / "dashboard"
BUDGETS = {
    t["id"]: t.get("budget_tokens")
    for t in json.loads((EVALS / "tasks.json").read_text())["pilot"]["tasks"]
}
CATEGORY = {
    t["id"]: t.get("category", "")
    for t in json.loads((EVALS / "tasks.json").read_text())["pilot"]["tasks"]
}


def parse_sessions(cell_dir: Path):
    messages = []
    for f in sorted(cell_dir.glob("session/*.jsonl")):
        for line in f.read_text(errors="replace").splitlines():
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if rec.get("type") != "message":
                continue
            m = rec.get("message") or {}
            role = m.get("role") or "system"
            items = []
            content = m.get("content")
            if isinstance(content, str):
                items.append({"kind": "text", "text": content})
            elif isinstance(content, list):
                for c in content:
                    t = c.get("type")
                    if t == "text":
                        items.append({"kind": "text", "text": c.get("text", "")})
                    elif t == "thinking":
                        items.append({"kind": "thinking", "text": c.get("text", "")})
                    elif t == "toolCall":
                        items.append({
                            "kind": "toolcall",
                            "name": c.get("name", "?"),
                            "args": json.dumps(c.get("arguments"), ensure_ascii=False)[:600],
                        })
            usage = m.get("usage") or {}
            out_usage = None
            if usage:
                out_usage = {
                    "in": usage.get("input", 0),
                    "out": usage.get("output", 0),
                }
            messages.append({"role": role, "items": items, "usage": out_usage})
    return messages


def cell_record(cell_dir: Path, cell_id: str, run_id: str):
    meta_p = cell_dir / "meta.json"
    if not meta_p.exists():
        return None
    meta = json.loads(meta_p.read_text())
    grades_p = RUNS / run_id / "output" / "grades.json"
    checks = []
    passed = False
    if grades_p.exists():
        g = json.loads(grades_p.read_text())["runs"].get(cell_id, {})
        checks = g.get("checks", [])
        passed = g.get("passed", False)
    tokens = int(meta.get("tokens_total") or 0)
    if not tokens:
        for f in cell_dir.glob("session/*.jsonl"):
            for line in f.read_text(errors="replace").splitlines():
                try:
                    u = (json.loads(line).get("message") or {}).get("usage") or {}
                except json.JSONDecodeError:
                    continue
                tokens += (u.get("input") or 0) + (u.get("output") or 0)
    violation = []
    vp = cell_dir / "AUDIT_VIOLATION"
    if vp.exists():
        violation = vp.read_text().strip().split(", ")
    budget = BUDGETS.get(meta.get("task", ""))
    over = bool(budget and tokens > budget)
    return {
        "task": meta.get("task"),
        "condition": meta.get("condition"),
        "checks": checks,
        "passed": passed and not violation and not over,
        "textual_passed": passed,
        "duration": meta.get("duration_s"),
        "tokens": tokens,
        "returncode": meta.get("returncode"),
        "timed_out": bool(meta.get("timed_out")),
        "violations": violation,
        "over_budget": over,
        "budget": budget,
    }


TEMPLATE = r"""<!doctype html>
<html lang="zh">
<head>
<meta charset="utf-8">
<title>Notist CLI Eval Dashboard</title>
<style>
:root{--bg:#0f1117;--panel:#171a23;--fg:#d6d9e0;--dim:#8a90a0;--ok:#4cc38a;--bad:#e5484d;
--warn:#f5a524;--accent:#6ea8fe;--line:#2a2f3d}
*{box-sizing:border-box}
body{background:var(--bg);color:var(--fg);font:14px/1.5 ui-sans-serif,system-ui,"Noto Sans SC";margin:0;padding:20px}
h1{font-size:18px;margin:0 0 12px}
select,input[type=file]{background:var(--panel);color:var(--fg);border:1px solid var(--line);border-radius:6px;padding:6px 10px}
.bar{display:flex;gap:12px;align-items:center;margin-bottom:14px;flex-wrap:wrap}
.cards{display:flex;gap:14px;margin:10px 0 18px;flex-wrap:wrap}
.card{background:var(--panel);border:1px solid var(--line);border-radius:8px;padding:10px 16px;min-width:110px}
.card .v{font-size:22px;font-weight:600}
.card .l{color:var(--dim);font-size:12px}
table{border-collapse:collapse;width:100%;background:var(--panel);border-radius:8px;overflow:hidden}
th,td{border-bottom:1px solid var(--line);padding:8px 12px;text-align:left;font-size:13px}
th{color:var(--dim);font-weight:500;background:#1b1f2b}
td.cell{cursor:pointer;font-weight:600}
.pass{color:var(--ok)} .fail{color:var(--bad)} .timeout{color:var(--warn)}
tr:hover td{background:#1d2230}
.tag{font-size:11px;border:1px solid var(--line);border-radius:4px;padding:1px 6px;color:var(--dim);margin-left:6px}
.flip{outline:2px solid var(--warn);border-radius:4px}
#modal{position:fixed;inset:0;background:rgba(0,0,0,.6);display:none;align-items:flex-start;justify-content:center;overflow:auto;padding:40px 16px}
#modal .box{background:var(--panel);border:1px solid var(--line);border-radius:10px;max-width:900px;width:100%;padding:20px}
.checks li{margin:4px 0}
.answer{white-space:pre-wrap;background:#0c0e14;border:1px solid var(--line);border-radius:8px;padding:12px;max-height:300px;overflow:auto}
.msg{border:1px solid var(--line);border-radius:8px;margin:8px 0;padding:8px 12px}
.msg.user{border-left:3px solid var(--accent)}
.msg.assistant{border-left:3px solid var(--ok)}
.msg.toolResult{border-left:3px solid var(--dim)}
.msg .role{color:var(--dim);font-size:11px;text-transform:uppercase;letter-spacing:.5px}
.msg details summary{cursor:pointer;color:var(--dim);font-size:12px}
.toolcall{display:inline-block;background:#20263a;border-radius:6px;padding:2px 8px;margin:2px;color:var(--accent);font-size:12px}
.tokbar{height:8px;background:#20263a;border-radius:4px;min-width:60px}
.tokbar>i{display:block;height:100%;border-radius:4px;background:var(--accent)}
.hide{display:none}
a{color:var(--accent)}
</style>
</head>
<body>
<h1>Notist CLI Eval Dashboard</h1>
<div class="bar">
  <label>运行 <select id="runA"></select></label>
  <label id="runBWrap" class="hide">对比 <select id="runB"></select></label>
  <label><input type="checkbox" id="diffMode"> 对比模式</label>
  <span id="status" style="color:var(--dim)"></span>
</div>
<div class="cards" id="cards"></div>
<table id="matrix"><thead></thead><tbody></tbody></table>
<div id="modal"><div class="box" id="mbox"></div></div>
<script>window.onerror=function(m,s,l,c,e){document.title="ERR:"+m+" @"+l+":"+c;};</script>
<script src="data.js"></script>
<script>
const D=window.EVAL_DASH;
const runNames=Object.keys(D.runs);
const $=s=>document.querySelector(s);
function esc(s){return (s??"").toString().replace(/[&<>"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;"}[c]))}
function tok(n){return n>=1000?(n/1000).toFixed(1)+"k":n}
function cellState(c){
  if(!c) return {cls:"",label:"—"};
  let cls=c.passed?"pass":"fail";
  if(c.timed_out||(c.returncode===null))cls="timeout";
  return {cls,label:(c.passed?"✓":"✗")+(c.timed_out?"⏱":"")+(c.violations.length?"⚠":"")+(c.over_budget?"¢":"")};
}
function fillRuns(){
  runNames.forEach(n=>{["#runA","#runB"].forEach(s=>{
    const o=document.createElement("option");o.value=n;o.textContent=n;$(s).appendChild(o);});});
  $("#runA").value=runNames[runNames.length-1];
  $("#runB").value=runNames[runNames.length-2]||runNames[0];
}
function conditions(run){return Object.keys(run.cells).sort();}
function tasks(run){const s=new Set();for(const c of Object.values(run.cells))for(const t of Object.keys(c))s.add(t);return [...s].sort();}
function render(){
  const a=D.runs[$("#runA").value];
  const b=$("#diffMode").checked?D.runs[$("#runB").value]:null;
  $("#runBWrap").classList.toggle("hide",!$("#diffMode").checked);
  const conds=conditions(a);
  const tks=tasks(a);
  // cards
  let P=0,N=0,TOK=0,DUR=0;
  for(const c of conds)for(const t of tks){const cell=a.cells[c]?.[t];if(!cell)continue;N++;P+=cell.passed?1:0;TOK+=cell.tokens||0;DUR+=cell.duration||0;}
  $("#cards").innerHTML=
    `<div class="card"><div class="v">${P}/${N}</div><div class="l">通过格数</div></div>`+
    `<div class="card"><div class="v">${tok(TOK)}</div><div class="l">总 tokens</div></div>`+
    `<div class="card"><div class="v">${Math.round(DUR/60)}m</div><div class="l">总时长</div></div>`+
    (b?(()=>{let P2=0,N2=0;for(const c of conditions(b))for(const t of tasks(b)){const cell=b.cells[c]?.[t];if(!cell)continue;N2++;P2+=cell.passed?1:0;}
      return `<div class="card"><div class="v">${P2}/${N2}</div><div class="l">对比 ${esc($("#runB").value)}</div></div>`;})():"");
  // matrix
  const thead=`<tr><th>任务</th>`+conds.map(c=>`<th>${esc(c)}</th>`).join("")+`</tr>`;
  let rows="";
  for(const t of tks){
    rows+=`<tr><td>${esc(t)}<span class="tag">${esc((D.tasks[t]||{}).category||"")}</span></td>`;
    for(const c of conds){
      const cell=a.cells[c]?.[t];
      const st=cellState(cell);
      let flip="";
      if(b){
        const pb=b.cells[c]?.[t]?.passed||false;
        const pa=cell?cell.passed:false;
        if(pa!==pb)flip=" flip";
      }
      const max=Math.max(1,...conds.map(cc=>a.cells[cc]?.[t]?.tokens||0));
      const bar=cell?`<div class="tokbar"><i style="width:${Math.round(100*(cell.tokens||0)/max)}%"></i></div>`:"";
      rows+=`<td class="cell ${st.cls}${flip}" data-run="${esc($("#runA").value)}" data-cell="${esc(c)}|${esc(t)}">
        ${st.label}<div style="color:var(--dim);font-size:11px">${cell?`tok ${tok(cell.tokens||0)} · ${cell.duration??"?"}s`:""}</div>${bar}</td>`;
    }
    rows+="</tr>";
  }
  $("#matrix").querySelector("thead").innerHTML=thead;
  $("#matrix").querySelector("tbody").innerHTML=rows;
  $("#status").textContent=b?`对比模式：黄框 = 通过状态翻转`:`点格子看详情`;
}
function showCell(runId,cond,taskId){
  const key=taskId+"__"+cond;
  const run=D.runs[runId];
  const cell=run.cells[cond][taskId];
  const checks=(cell.checks||[]).map(c=>`<li class="${c.passed?"pass":"fail"}">${c.passed?"✓":"✗"} ${esc(c.desc)}</li>`).join("");
  $("#mbox").innerHTML=`<h1>${esc(taskId)} <span class="tag">${esc(cond)}</span></h1>
   <p style="color:var(--dim)">${cell.passed?"通过":"未通过"} · ${cell.duration??"?"}s · ${cell.tokens} tokens
   ${cell.budget?` / 预算 ${tok(cell.budget)}`:""} ${cell.timed_out?"· 超时":""}
   ${cell.violations.length?"· 违规: "+esc(cell.violations.join(",")):""}</p>
   <ul class="checks">${checks||"<li>无判据记录</li>"}</ul>
   <h3 style="margin:14px 0 6px">最终回答</h3><div class="answer" id="ans">加载中…</div>
   <h3 style="margin:14px 0 6px">会话回放 <button onclick="loadSession('${esc(runId)}','${esc(key)}')">加载</button></h3>
   <div id="sess"></div>
   <p><button onclick="document.getElementById('modal').style.display='none'">关闭</button></p>`;
  $("#modal").style.display="flex";
  try{history.replaceState(null,"","#"+encodeURIComponent(runId)+"|"+encodeURIComponent(key));}catch(e){}
  fetchCellAnswer(runId,key);
  loadSession(runId,key);
}
function fetchCellAnswer(runId,key){
  // answer embedded in data.js for the latest 200 cells only; else lazy script
  const a=D.answers[runId+"||"+key];
  if(a!==undefined){$("#ans").textContent=a||"(空)";return;}
  const s=document.createElement("script");
  s.src=`answers/${encodeURIComponent(runId)}/${encodeURIComponent(key)}.js`;
  s.onload=()=>{$("#ans").textContent=window.EVAL_ANSWERS[runId+"||"+key]||"(空)";};
  s.onerror=()=>{$("#ans").textContent="(无法加载)";};
  document.body.appendChild(s);
}
function loadSession(runId,key){
  $("#sess").textContent="加载中…";
  const s=document.createElement("script");
  s.src=`sessions/${encodeURIComponent(runId)}/${encodeURIComponent(key)}.js`;
  s.onload=()=>{
    const msgs=window.EVAL_SESSIONS[runId+"||"+key]||[];
    $("#sess").innerHTML=msgs.map(m=>{
      const body=m.items.map(it=>{
        if(it.kind==="text")return `<div>${esc(it.text)}</div>`;
        if(it.kind==="thinking")return `<details><summary>thinking</summary><div style="color:var(--dim);white-space:pre-wrap">${esc(it.text)}</div></details>`;
        if(it.kind==="toolcall")return `<span class="toolcall">🔧 ${esc(it.name)} ${esc(it.args||"")}</span>`;
        return "";
      }).join("");
      const u=m.usage?`<span style="float:right;color:var(--dim);font-size:11px">in ${m.usage.in} / out ${m.usage.out}</span>`:"";
      return `<div class="msg ${esc(m.role)}"><div class="role">${esc(m.role)} ${u}</div>${body}</div>`;
    }).join("")||"(空会话)";
  };
  s.onerror=()=>{$("#sess").textContent="(会话加载失败)";};
  document.body.appendChild(s);
}
document.addEventListener("click",e=>{
  const td=e.target.closest("td.cell");
  if(td){const[c,t]=td.dataset.cell.split("|");showCell(td.dataset.run,c,t);}
  if(e.target.id==="modal")e.target.style.display="none";
});
$("#runA").addEventListener("change",render);
$("#runB").addEventListener("change",render);
$("#diffMode").addEventListener("change",render);
function openFromHash(){
  const h=decodeURIComponent(location.hash.slice(1));
  if(!h)return;
  const [r,key]=h.split("|");
  if(!r||!key||!D.runs[r])return;
  const ti=key.lastIndexOf("__");
  if(ti<0)return;
  const task=key.slice(0,ti), cond=key.slice(ti+2);
  if(!D.runs[r].cells[cond]?.[task])return;
  $("#runA").value=r;render();showCell(r,cond,task);
}
fillRuns();render();openFromHash();
window.addEventListener("hashchange",openFromHash);
</script>
</body>
</html>"""


def main() -> None:
    if OUT.exists():
        shutil.rmtree(OUT)
    (OUT / "sessions").mkdir(parents=True)
    (OUT / "answers").mkdir(parents=True)

    runs = {}
    run_dirs = sorted((p for p in RUNS.iterdir() if p.is_dir()),
                      key=lambda p: p.stat().st_mtime)
    for run_dir in run_dirs:
        grades_p = run_dir / "output" / "grades.json"
        if not grades_p.exists():
            continue
        run_id = run_dir.name
        cells: dict = {}
        for cell_dir in sorted(p for p in (run_dir / "output").iterdir() if p.is_dir()):
            cell_id = cell_dir.name
            rec = cell_record(cell_dir, cell_id, run_id)
            if not rec:
                continue
            cond, task = rec["condition"], rec["task"]
            cells.setdefault(cond, {})[task] = rec
            # lazy payloads
            answer = (cell_dir / "answer.txt").read_text(errors="replace")
            ans_dir = OUT / "answers" / run_id
            ans_dir.mkdir(exist_ok=True)
            (ans_dir / f"{cell_id}.js").write_text(
                f'window.EVAL_ANSWERS[{json.dumps(run_id + "||" + cell_id)}]='
                f'{json.dumps(answer, ensure_ascii=False)};\n')
            sess = parse_sessions(cell_dir)
            sess_dir = OUT / "sessions" / run_id
            sess_dir.mkdir(exist_ok=True)
            (sess_dir / f"{cell_id}.js").write_text(
                f'window.EVAL_SESSIONS[{json.dumps(run_id + "||" + cell_id)}]='
                f'{json.dumps(sess, ensure_ascii=False)};\n')
        runs[run_id] = {"cells": cells}

    answers = {}
    ans_root = OUT / "answers"
    # inline answers of the newest run for instant modal; others lazy
    newest = max(runs, key=lambda r: (RUNS / r).stat().st_mtime)
    for cell_dir in (RUNS / newest / "output").iterdir():
        if not cell_dir.is_dir():
            continue
        a = (cell_dir / "answer.txt").read_text(errors="replace")
        answers[f"{newest}||{cell_dir.name}"] = a[:20000]

    (OUT / "data.js").write_text(
        "window.EVAL_DASH=" + json.dumps({"runs": runs, "tasks": CATEGORY, "answers": answers},
                                         ensure_ascii=False) + ";\n")
    (OUT / "index.html").write_text(TEMPLATE, encoding="utf-8")
    size = sum(f.stat().st_size for f in OUT.rglob("*") if f.is_file())
    print(f"dashboard built: {OUT} ({size//1024} KiB, {len(runs)} runs)")


if __name__ == "__main__":
    main()
