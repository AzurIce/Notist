import { byteIndexCache, charIndexForByte } from "./bytes.js";

// —— 全局状态 ——

const state = {
  files: new Map(),     // path -> .notc 文本（textarea 编辑写回这里）
  binaries: new Map(),  // path -> base64
  entry: null,
  snapshot: null,
  requestSeq: 0,
  worker: null,
  autoTimer: null,
  dependencies: {},
  filters: { kinds: new Set(), name: "", errorsOnly: false },
  nodeBudget: { hit: false },
};

const FIXTURE_ENTRY = ["playground.notc", "literals.notc", "parameters.notc", "recovery.notc"];

const $ = (id) => document.getElementById(id);
const els = {
  entry: $("entry"),
  analyze: $("analyze"),
  auto: $("auto"),
  cancel: $("cancel"),
  openDir: $("open-dir"),
  download: $("download"),
  status: $("status"),
  editor: $("editor"),
  sourceFile: $("source-file"),
  sourceView: $("source-view"),
  tabs: $("tabs"),
  panels: {
    tokens: $("panel-tokens"),
    ast: $("panel-ast"),
    eval: $("panel-eval"),
    result: $("panel-result"),
  },
};

// —— 请求构造 ——

// files key 按字典序插入：language core 按 files 迭代顺序分配 source_id
// （serde_json 字典序），key 顺序不同 → source_id 不同 → 快照不同。
function buildRequest() {
  const files = {};
  for (const key of [...state.files.keys()].sort()) files[key] = state.files.get(key);
  const binaries = {};
  for (const key of [...state.binaries.keys()].sort()) binaries[key] = state.binaries.get(key);
  return {
    request_id: `web-${++state.requestSeq}`,
    entry: state.entry,
    files,
    binaries,
    dependencies: state.dependencies,
    options: {},
  };
}

// —— Worker 生命周期 ——

function ensureWorker() {
  if (state.worker) return state.worker;
  const worker = new Worker("./worker.js", { type: "module" });
  worker.onmessage = (event) => onWorkerMessage(event.data);
  worker.onerror = (event) => setStatus(`worker 错误：${event.message || event.type}`);
  state.worker = worker;
  return worker;
}

function destroyWorker() {
  if (state.worker) {
    state.worker.terminate();
    state.worker = null;
  }
}

function runAnalysis() {
  // 旧 Worker 连旧请求一起丢弃，保证旧快照不会覆盖更新的请求。
  destroyWorker();
  const worker = ensureWorker();
  const request = buildRequest();
  setStatus(`分析中…（${request.request_id}）`);
  worker.postMessage({ type: "analyze", request });
}

function onWorkerMessage(message) {
  if (!message) return;
  if (message.type === "snapshot") {
    state.snapshot = JSON.parse(message.json);
    renderAll();
    renderStatusLine();
  } else if (message.type === "failed") {
    setStatus(`请求 ${message.request_id} 失败：${message.message}`);
  }
}

// —— 状态行 ——

function setStatus(text) {
  els.status.textContent = text;
}

function renderStatusLine() {
  const snap = state.snapshot;
  if (!snap) return;
  const stats = snap.result.stats;
  const truncated = snap.truncated.length ? `，截断：${snap.truncated.join("/")}` : "";
  setStatus(
    `${snap.request_id} · ${snap.platform.elapsed_ms.toFixed(1)}ms · ` +
      `tokens ${stats.tokens} · events ${stats.events}${truncated}`
  );
}

// —— 载入 fixtures / 目录 ——

async function fetchText(path) {
  const response = await fetch(`../fixtures/${path}`);
  if (!response.ok) throw new Error(`fixtures/${path}: ${response.status}`);
  return response.text();
}

async function fetchBase64(path) {
  const response = await fetch(`../fixtures/${path}`);
  if (!response.ok) throw new Error(`fixtures/${path}: ${response.status}`);
  return bytesToBase64(await response.arrayBuffer());
}

function bytesToBase64(buffer) {
  const bytes = new Uint8Array(buffer);
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary);
}

async function loadFixtures() {
  state.files.clear();
  state.binaries.clear();
  for (const path of FIXTURE_ENTRY) {
    state.files.set(path, await fetchText(path));
  }
  const packageRequest = await fetch('../fixtures/package.json').then(r => r.json());
  for (const [path, text] of Object.entries(packageRequest.files)) state.files.set(path, text);
  for (const [path, bytes] of Object.entries(packageRequest.binaries)) state.binaries.set(path, bytes);
  state.dependencies = packageRequest.dependencies;
  state.entry = packageRequest.entry;
  rebuildEntryOptions();
  loadEntryIntoEditor();
  renderSourceSelector();
  runAnalysis();
}

async function collectDirectory(dirHandle, prefix, paths) {
  for await (const [name, handle] of dirHandle.entries()) {
    const path = prefix ? `${prefix}/${name}` : name;
    if (handle.kind === "directory") {
      await collectDirectory(handle, path, paths);
    } else if (name.endsWith(".notc")) {
      paths.notc.push(path);
      state.files.set(path, await (await handle.getFile()).text());
    } else if (name.endsWith(".wasm")) {
      paths.wasm.push(path);
      state.binaries.set(path, bytesToBase64(await (await handle.getFile()).arrayBuffer()));
    }
  }
}

async function openDirectory() {
  const dirHandle = await window.showDirectoryPicker();
  state.files.clear();
  state.binaries.clear();
  state.dependencies = {};
  const paths = { notc: [], wasm: [] };
  await collectDirectory(dirHandle, "", paths);
  if (!paths.notc.length) {
    setStatus("目录里没有 .notc 文件");
    return;
  }
  // 入口候选：顶层 .notc（不在 packages/ 下）优先，否则按字典序第一个。
  const topLevel = paths.notc.filter((p) => !p.includes("/"));
  state.entry = (topLevel.length ? topLevel : paths.notc)[0];
  rebuildEntryOptions();
  loadEntryIntoEditor();
  renderSourceSelector();
  runAnalysis();
}

// —— 入口与编辑区 ——

function rebuildEntryOptions() {
  const entries = [...state.files.keys()].sort();
  if (state.entry && !entries.includes(state.entry)) state.entry = entries[0];
  els.entry.replaceChildren(
    ...entries.map((path) => {
      const option = document.createElement("option");
      option.value = path;
      option.textContent = path;
      return option;
    })
  );
  els.entry.value = state.entry;
}

function loadEntryIntoEditor() {
  els.editor.value = state.files.get(state.entry) ?? "";
}

function scheduleAutoAnalysis() {
  if (!els.auto.checked) return;
  clearTimeout(state.autoTimer);
  state.autoTimer = setTimeout(runAnalysis, 300);
}

// —— Source 面板（range 高亮） ——

function renderSourceSelector() {
  const snap = state.snapshot;
  const paths = snap
    ? snap.source.files.map((f) => f.path)
    : [...state.files.keys()].sort();
  els.sourceFile.replaceChildren(
    ...paths.map((path) => {
      const option = document.createElement("option");
      option.value = path;
      option.textContent = path;
      return option;
    })
  );
  if (!paths.includes(els.sourceFile.value)) els.sourceFile.value = state.entry;
  renderSource();
}

function renderSource() {
  const text = state.files.get(els.sourceFile.value) ?? "";
  els.sourceView.replaceChildren(document.createTextNode(text));
}

function highlightRange(sourceId, range) {
  if (!range) return;
  const snap = state.snapshot;
  const file = snap && snap.source.files.find((f) => f.source_id === sourceId);
  const path = file ? file.path : els.sourceFile.value;
  if (els.sourceFile.value !== path) {
    els.sourceFile.value = path;
    renderSource();
  }
  const text = state.files.get(path) ?? "";
  const offsets = byteIndexCache(text);
  const start = charIndexForByte(offsets, range[0]);
  const end = charIndexForByte(offsets, range[1]);
  const mark = document.createElement("mark");
  if (end > start) {
    mark.textContent = text.slice(start, end);
  } else {
    // 零宽 range（如插入点诊断）：给一个可见光标。
    mark.className = "zero-width";
    mark.textContent = "​";
  }
  els.sourceView.replaceChildren(
    document.createTextNode(text.slice(0, start)),
    mark,
    document.createTextNode(text.slice(end))
  );
  mark.scrollIntoView({ block: "nearest" });
}

// 点击任何带 data-hl 的元素 → 高亮对应源文 range。
document.addEventListener("click", (event) => {
  const target = event.target.closest("[data-hl]");
  if (!target) return;
  highlightRange(Number(target.dataset.sourceId), JSON.parse(target.dataset.range));
});

// —— 渲染小件 ——

function hlAttrs(sourceId, range) {
  return ` data-hl data-source-id="${sourceId}" data-range='${JSON.stringify(range)}'`;
}

function clickableRow(sourceId, range, className = "") {
  const row = document.createElement("div");
  row.className = `row clickable ${className}`.trim();
  if (range) {
    row.dataset.hl = "";
    row.dataset.sourceId = sourceId;
    row.dataset.range = JSON.stringify(range);
  }
  return row;
}

function span(className, text) {
  const node = document.createElement("span");
  node.className = className;
  node.textContent = text;
  return node;
}

// 大数组窗口化：先渲染前 windowSize 条，剩余用「显示更多」分块展开。
function windowedList(container, items, renderItem, windowSize = 500) {
  let shown = 0;
  const more = document.createElement("button");
  more.className = "more";
  const draw = () => {
    const until = Math.min(shown + windowSize, items.length);
    for (let i = shown; i < until; i++) {
      const node = renderItem(items[i], i);
      if (more.parentNode === container) container.insertBefore(node, more);
      else container.append(node);
    }
    shown = until;
    if (shown >= items.length) {
      more.remove();
    } else {
      more.textContent = `显示更多（${items.length - shown} / ${items.length}）`;
      if (more.parentNode !== container) container.append(more);
    }
  };
  more.onclick = draw;
  draw();
}

// —— Tokens 面板 ——

function renderTokens() {
  const panel = els.panels.tokens;
  panel.replaceChildren();
  const tokens = state.snapshot?.syntax.tokens ?? [];
  const head = document.createElement("div");
  head.className = "row head";
  head.append(span("c-i", "i"), span("c-kind", "kind"), span("c-text", "text"), span("c-range", "range"), span("c-rec", "rec"));
  panel.append(head);
  windowedList(panel, tokens, (token) => {
    const row = clickableRow(token.source_id, token.range, token.recovery ? "recovery" : "");
    row.append(
      span("c-i", String(token.i)),
      span("c-kind", token.kind),
      span("c-text", JSON.stringify(token.text)),
      span("c-range", `[${token.range[0]}, ${token.range[1]})`),
      span("c-rec", token.recovery ? "⚠" : "")
    );
    return row;
  });
}

// —— AST 面板 ——

const EXPR_KEYS = {
  none: [], string: ["value"], int: ["value"], bool: ["value"], name: ["name"],
  list: ["items"], dict: ["fields"], content: ["parts"], styled: ["parts"],
  call: ["callee", "args"], field: ["base"], lambda: ["params", "body"],
  if: ["condition", "then", "else"], binary: ["left", "right"],
};

function exprSummary(expr) {
  switch (expr.kind) {
    case "name": return `name ${expr.name}`;
    case "string": return `string ${JSON.stringify(expr.value)}`;
    case "int": case "bool": return `${expr.kind} ${String(expr.value)}`;
    case "binary": return `binary ${expr.op}`;
    case "field": return `field .${expr.field}`;
    case "lambda": return `lambda (${(expr.params ?? []).map((p) => p.name ?? p).join(", ")})`;
    case "call": return "call";
    default: return expr.kind;
  }
}

function renderExpr(expr, container, sourceId, depth) {
  if (state.nodeBudget.count++ > state.nodeBudget.limit) {
    state.nodeBudget.hit = true;
    return;
  }
  const row = clickableRow(sourceId, expr.range, "tree-node");
  row.style.setProperty("--depth", depth);
  row.append(span("c-kind", exprSummary(expr)), span("c-id", `#${expr.id}`));
  container.append(row);
  for (const key of EXPR_KEYS[expr.kind] ?? []) {
    const child = expr[key];
    if (Array.isArray(child)) {
      for (const item of child) {
        if (item && typeof item === "object" && "expr" in item) renderExpr(item.expr, container, sourceId, depth + 1);
        else if (item && typeof item === "object" && "value" in item && item.value && typeof item.value === "object" && "kind" in item.value) renderExpr(item.value, container, sourceId, depth + 1);
        else if (item && typeof item === "object" && "kind" in item) renderExpr(item, container, sourceId, depth + 1);
      }
    } else if (child && typeof child === "object" && "kind" in child) {
      renderExpr(child, container, sourceId, depth + 1);
    }
  }
}

function renderStatement(stmt, container) {
  if (state.nodeBudget.count++ > state.nodeBudget.limit) {
    state.nodeBudget.hit = true;
    return;
  }
  const label = stmt.kind === "let" ? `let ${stmt.name}`
    : stmt.kind === "use" ? `use ${JSON.stringify(stmt.imports)}`
    : stmt.kind === "wasm" ? `wasm ${stmt.path}`
    : stmt.kind;
  const row = clickableRow(stmt.source_id, stmt.range, "tree-node stmt");
  row.style.setProperty("--depth", 0);
  row.append(span("c-kind", label), span("c-id", `#${stmt.id}`));
  container.append(row);
  const bodyExpr = stmt.body?.expr ?? stmt.expr;
  if (bodyExpr) renderExpr(bodyExpr, container, stmt.source_id, 1);
}

function renderAst() {
  const panel = els.panels.ast;
  panel.replaceChildren();
  const syntax = state.snapshot?.syntax;
  if (!syntax) return;
  state.nodeBudget = { count: 0, limit: 2000, hit: false };
  const list = document.createElement("div");
  for (const stmt of syntax.statements) renderStatement(stmt, list);
  if (state.nodeBudget.hit) list.append(span("muted", "…（节点过多，已省略）"));
  panel.append(list);
  if (syntax.errors.length) {
    const head = document.createElement("div");
    head.className = "subhead";
    head.textContent = `syntax.errors（${syntax.errors.length}）`;
    panel.append(head);
    for (const error of syntax.errors) {
      const row = clickableRow(error.source_id, error.range, "diag error");
      row.append(span("c-kind", `[${error.range[0]}, ${error.range[1]})`), span("c-text", error.message));
      panel.append(row);
    }
  }
}

// —— Evaluation 面板 ——

function renderEvalFilters(events) {
  const bar = $("eval-filters");
  bar.replaceChildren();
  const kinds = [...new Set(events.map((e) => e.kind))].sort();
  for (const kind of kinds) {
    const label = document.createElement("label");
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = !state.filters.kinds.has(kind);
    box.onchange = () => {
      // kinds 是排除集：空 = 全选，取消勾选 = 排除该 kind。
      if (box.checked) state.filters.kinds.delete(kind);
      else state.filters.kinds.add(kind);
      renderEval();
    };
    label.append(box, ` ${kind}`);
    bar.append(label);
  }
  const name = document.createElement("input");
  name.placeholder = "name 子串过滤";
  name.value = state.filters.name;
  name.oninput = () => {
    state.filters.name = name.value;
    renderEval();
  };
  const errorsOnly = document.createElement("label");
  const errorsBox = document.createElement("input");
  errorsBox.type = "checkbox";
  errorsBox.checked = state.filters.errorsOnly;
  errorsBox.onchange = () => {
    state.filters.errorsOnly = errorsBox.checked;
    renderEval();
  };
  errorsOnly.append(errorsBox, " 仅错误");
  bar.append(name, errorsOnly);
}

function eventMatches(event) {
  const f = state.filters;
  if (f.errorsOnly && event.kind !== "error") return false;
  if (f.kinds.size && !f.kinds.has(event.kind)) return false;
  if (f.name && !(event.name ?? "").includes(f.name)) return false;
  return true;
}

function renderEval() {
  const panel = els.panels.eval;
  const events = state.snapshot?.evaluation.events ?? [];
  renderEvalFilters(events);
  const list = document.createElement("div");
  list.className = "eval-list";
  const head = document.createElement("div");
  head.className = "row head";
  head.append(span("c-i", "i"), span("c-kind", "kind"), span("c-name", "name"), span("c-detail", "detail"), span("c-io", "in → out"));
  list.append(head);
  const matched = events.filter(eventMatches);
  windowedList(list, matched, (event) => {
    const row = clickableRow(event.source_id, event.range, `ev-${event.kind}`);
    row.append(
      span("c-i", String(event.i)),
      span("c-kind", event.kind),
      span("c-name", event.name ?? ""),
      span("c-detail", event.detail ?? ""),
      span("c-io", `${event.in ?? ""} → ${event.out ?? ""}`)
    );
    return row;
  });
  panel.replaceChildren($("eval-filters"), list);
}

// —— Normalization 面板 ——

function summarizeValue(value) {
  const text = JSON.stringify(value);
  return text.length > 120 ? `${text.slice(0, 119)}…` : text;
}

function renderContentNode(node, container, depth) {
  if (state.nodeBudget.count++ > state.nodeBudget.limit) {
    state.nodeBudget.hit = true;
    return;
  }
  const row = document.createElement("div");
  row.className = "tree-node content-node";
  row.style.setProperty("--depth", depth);
  let label;
  if (node == null) label = "null";
  else if (node.truncated) label = `…（深度截断）`;
  else if (typeof node.text === "string") label = `text ${JSON.stringify(node.text.length > 60 ? node.text.slice(0, 59) + "…" : node.text)}`;
  else if (node.sequence) label = `sequence（${node.sequence.length}）`;
  else if (node.item) label = `item ${node.item}`;
  else if (node.error) label = `error ${node.error}`;
  else label = JSON.stringify(node).slice(0, 80);
  row.append(span("c-kind", label));
  if (node != null && node.id != null) row.append(span("c-id", `#${node.id}`));
  container.append(row);
  const children = node?.sequence ?? (node?.args ? Object.entries(node.args).map(([key, value]) => ({ dict: { [key]: value } })) : node?.dict ? Object.values(node.dict) : Array.isArray(node) ? node : null);
  if (Array.isArray(children)) {
    for (const child of children) renderContentNode(child, container, depth + 1);
  }
}

// —— Result 面板 ——

const SEVERITY_CLASS = { warning: "warning", error: "error", failure: "failure" };

function renderResult() {
  const panel = els.panels.result;
  panel.replaceChildren();
  const result = state.snapshot?.result;
  if (!result) return;

  const contentHead = document.createElement("div");
  contentHead.className = "subhead";
  contentHead.textContent = "content";
  const contentTree = document.createElement("div");
  state.nodeBudget = { count: 0, limit: 2000, hit: false };
  renderContentNode(result.content, contentTree, 0);

  const bindHead = document.createElement("div");
  bindHead.className = "subhead";
  bindHead.textContent = `bindings（${result.bindings.length}）`;
  const bindList = document.createElement("div");
  for (const binding of result.bindings) {
    const row = document.createElement("div");
    row.className = "row";
    row.append(span("c-name", binding.name), span("c-io", binding.value), span("muted", ` source ${binding.source_id}`));
    bindList.append(row);
  }

  const diagHead = document.createElement("div");
  diagHead.className = "subhead";
  diagHead.textContent = `diagnostics（${result.diagnostics.length}）`;
  const diagList = document.createElement("div");
  for (const diag of result.diagnostics) {
    const row = clickableRow(diag.source_id, diag.range, `diag ${SEVERITY_CLASS[diag.severity] ?? ""}`);
    row.append(
      span("severity", diag.severity),
      span("muted", ` [${diag.stage}]`),
      span("c-text", diag.message),
      span("muted", ` (${diag.source_id}: ${diag.range?.join("-") ?? ""})`)
    );
    diagList.append(row);
  }

  const stats = document.createElement("div");
  stats.className = "row stats";
  for (const [key, value] of Object.entries(result.stats)) {
    stats.append(span("muted", ` ${key}=${value}`));
  }
  if (state.snapshot.truncated.length) {
    stats.append(span("warning", ` truncated: ${state.snapshot.truncated.join(", ")}`));
  }

  panel.append(contentHead, contentTree, bindHead, bindList, diagHead, diagList, stats);
}

// —— 总渲染 / 下载 / 事件绑定 ——

function renderAll() {
  renderSourceSelector();
  renderTokens();
  renderAst();
  renderEval();
  renderResult();
}

function downloadSnapshot() {
  if (!state.snapshot) return;
  const blob = new Blob([JSON.stringify(state.snapshot, null, 2)], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = `notist-snapshot-${state.snapshot.request_id}.json`;
  link.click();
  URL.revokeObjectURL(url);
}

els.analyze.onclick = runAnalysis;
els.cancel.onclick = () => {
  destroyWorker();
  setStatus("已取消");
};
els.auto.onchange = () => {
  if (els.auto.checked) runAnalysis();
};
els.entry.onchange = () => {
  state.entry = els.entry.value;
  loadEntryIntoEditor();
  runAnalysis();
};
els.editor.oninput = () => {
  state.files.set(state.entry, els.editor.value);
  scheduleAutoAnalysis();
};
els.sourceFile.onchange = renderSource;
els.download.onclick = downloadSnapshot;
els.tabs.addEventListener("click", (event) => {
  const button = event.target.closest("button[data-tab]");
  if (!button) return;
  for (const [name, panel] of Object.entries(els.panels)) {
    panel.hidden = name !== button.dataset.tab;
  }
  for (const tab of els.tabs.querySelectorAll("button")) {
    tab.classList.toggle("active", tab === button);
  }
});

// File System Access API 仅在 Chromium 系可用；不支持时隐藏入口。
if (!window.showDirectoryPicker) els.openDir.hidden = true;
els.openDir.onclick = () => openDirectory().catch((error) => setStatus(`打开目录失败：${error.message}`));

loadFixtures().catch((error) => setStatus(`载入 fixtures 失败：${error.message}`));
