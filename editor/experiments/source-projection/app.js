import { Editor, Node, Extension } from "@tiptap/core";
import StarterKit from "@tiptap/starter-kit";
import { Plugin } from "@tiptap/pm/state";
import { EditorState, Annotation, StateEffect, StateField } from "@codemirror/state";
import { EditorView, keymap, lineNumbers, drawSelection, highlightActiveLine, Decoration, ViewPlugin } from "@codemirror/view";
import { defaultKeymap } from "@codemirror/commands";
import initCore, { EditorDocument } from "/kernel/notist_editor_core_wasm.js";
import { EditorCore } from "../../core/index.mjs";
import { project, serialize, withPositions, sourcePatch, richToSource, sourceToRich, byteToUTF16 } from "./projection.mjs";

const $ = selector => document.querySelector(selector);
const SAMPLE = `= 在文字里思考

让文档像纸一样自然，也像代码一样精确。

有些想法需要 *强调*，有些只需要 _轻轻带过_。写作时，先把注意力留给文字。

== 同一份文字，两种视角

在这里选中文字、加粗，或者按 Enter 分段。右边的源码会跟着变化；在右边修改文字，这里也会同步。

// 这条注释和空行会原样保留。
#let author = "写作者";

源码里的表达式、注解，以及尚未写完的语法，都可以继续留在原来的位置。
`;
const STORAGE = "notist-source-projection-draft";
const params = new URLSearchParams(location.search);
const errors = [];
let stored;
try { stored = localStorage.getItem(STORAGE); } catch {}
await initCore();
const createCore = text => EditorCore.create(EditorDocument, {
  identity: { document_id: "source-projection-draft", history_id: crypto.randomUUID() }, text,
  onListenerError(error) { errors.push(String(error)); console.error(error); },
});
let syncSession = null;
if (params.has("room")) {
  try {
    syncSession = await (await import("/sync/client.js")).openSyncDocument({
      EditorCore, EditorDocument, room: params.get("room"), replica: params.get("replica") || crypto.randomUUID(),
    });
  } catch (error) {
    const message = document.createElement("p"); message.textContent = error.message;
    document.querySelector("#workspace").replaceChildren(message);
    throw error;
  }
}
let core = syncSession?.core ?? createCore(stored ?? SAMPLE);
const source = () => core.snapshot().text;
let richVersion = core.snapshot().version, cmVersion = richVersion;
let projection = withPositions(project(source()));
let rich, cm, syncing = false, revision = 0, lastPatch = null;
let activeView = "rich", diagnostics = [], languageReady = false;

function observeCore() {
  core.subscribe(() => {
    // Read the current snapshot: multiple synchronous host writes may already
    // have committed before this microtask. Never rebuild the active rich view
    // for its own accepted input/composition transactions.
    syncCM();
    if (projection.source !== source()) syncRich();
    richVersion = core.snapshot().version;
    changed();
  });
}
observeCore();

let toastTimer;
function toast(message) {
  $("#toast").textContent = message; $("#toast").hidden = false;
  clearTimeout(toastTimer); toastTimer = setTimeout(() => { $("#toast").hidden = true; }, 4200);
}
function selection(view = activeView) {
  if (view === "source" && cm) return { view, anchor: cm.state.selection.main.anchor, head: cm.state.selection.main.head };
  const range = rich?.state.selection;
  return { view: "rich", anchor: range ? richToSource(projection, range.anchor) : 0, head: range ? richToSource(projection, range.head) : 0 };
}
function resolveSelection(event, fallback) {
  if (event.restored_metadata?.kind !== "projection-selection" || event.restored_positions.length !== 2) return fallback;
  return { view: event.restored_metadata.view, anchor: event.restored_positions[0], head: event.restored_positions[1] };
}
function commitSource(next, origin, before) {
  const patch = sourcePatch(source(), next);
  if (!patch) return;
  core.transact({ expectedVersion: origin === "rich" ? richVersion : cmVersion, edits: [patch], origin, undoMetadata: { kind: "projection-selection", view: before.view }, undoPositions: [before.anchor, before.head] });
  if (origin === "rich") richVersion = core.snapshot().version;
  else cmVersion = core.snapshot().version;
  lastPatch = { ...patch, origin };
}
function changed() {
  revision++;
  if (!syncSession) try { localStorage.setItem(STORAGE, source()); } catch {}
  refresh(); scheduleAnalysis();
}
function refresh() {
  $("#undo").disabled = !core.undoState.can_undo; $("#redo").disabled = !core.undoState.can_redo;
  $("#bold").classList.toggle("active", !!rich?.isActive("bold"));
  $("#italic").classList.toggle("active", !!rich?.isActive("italic"));
  $("#heading").classList.toggle("active", !!rich?.isActive("heading", { level: 2 }));
  $("#word-count").textContent = `${[...(rich?.getText() || "").replace(/\s/gu, "")].length} 字符`;
}
function compositionStart() { if (!core.undoState.group_open) core.beginUndoGroup(); }
function compositionEnd() { core.endUndoGroup(); }

const SourceIsland = Node.create({
  name: "sourceIsland", group: "block", atom: true, selectable: true, isolating: true,
  addAttributes() { return { rid: { default: null }, raw: { default: "" } }; },
  parseHTML() { return [{ tag: "div[data-source-island]" }]; },
  renderHTML({ node }) { return ["div", { "data-source-island": "", class: "source-island", contenteditable: "false" }, ["pre", {}, node.attrs.raw]]; },
  addNodeView() {
    return ({ node }) => {
      const dom = document.createElement("div"); dom.className = "source-island"; dom.contentEditable = "false";
      const header = document.createElement("div"); header.className = "island-heading";
      const label = document.createElement("span"); label.textContent = "‹/› 源码块";
      const button = document.createElement("button"); button.textContent = "在源码中编辑 ↗";
      button.setAttribute("aria-label", "在源码中编辑");
      const pre = document.createElement("pre"); pre.textContent = node.attrs.raw;
      header.append(label, button); dom.append(header, pre);
      button.addEventListener("click", () => {
        const block = projection.blocks.find(b => b.rid === node.attrs.rid);
        if (block) revealSource(block.from, block.to);
      });
      return { dom, stopEvent: event => event.target === button, ignoreMutation: () => true };
    };
  },
});
let beforeRichSelection;
const ProjectionBridge = Extension.create({
  name: "projectionBridge", priority: 2000,
  addGlobalAttributes() { return [{ types: ["paragraph", "heading"], attributes: { rid: { default: null, rendered: false } } }]; },
  addKeyboardShortcuts() {
    return { "Mod-z": () => { queueMicrotask(() => undo(false)); return true; }, "Mod-Shift-z": () => { queueMicrotask(() => undo(true)); return true; }, "Mod-y": () => { queueMicrotask(() => undo(true)); return true; } };
  },
  addProseMirrorPlugins() {
    return [new Plugin({ filterTransaction(transaction, state) {
      if (syncing || !transaction.docChanged) return true;
      try {
        if (JSON.stringify(richVersion) !== JSON.stringify(core.snapshot().version)) {
          queueMicrotask(() => syncRich());
          throw new Error("文档已有其他修改，视图同步后请重试。");
        }
        serialize(transaction.doc.toJSON(), projection);
        beforeRichSelection = { view: "rich", anchor: richToSource(projection, state.selection.anchor), head: richToSource(projection, state.selection.head) };
        return true;
      } catch (error) { queueMicrotask(() => toast(error.message)); return false; }
    } })];
  },
});

rich = new Editor({
  element: $("#rich-editor"), content: projection.doc,
  extensions: [StarterKit.configure({
    undoRedo: false, blockquote: false, bulletList: false, orderedList: false, listItem: false, listKeymap: false,
    code: false, codeBlock: false, hardBreak: false, horizontalRule: false, link: false, strike: false, underline: false, trailingNode: false,
    heading: { levels: [1, 2, 3] },
  }), SourceIsland, ProjectionBridge],
  // Markdown/HTML input rules would introduce a second, incompatible syntax.
  enableInputRules: false, enablePasteRules: false,
  editorProps: {
    attributes: { "aria-label": "Notist 文档编辑器", spellcheck: "false" },
    handleDOMEvents: { compositionstart: () => { compositionStart(); return false; }, compositionend: () => { compositionEnd(); return false; } },
  },
  onFocus() { activeView = "rich"; },
  onSelectionUpdate() { if (!syncing) { refresh(); highlightLinkedSelection(); } },
  onUpdate({ editor }) {
    if (syncing) return;
    const next = serialize(editor.getJSON(), projection);
    // splitBlock copies attributes. Give new blocks distinct identities without
    // replacing the document or interrupting the active contenteditable/IME.
    syncing = true;
    try {
      const tr = editor.state.tr;
      next.blocks.forEach(b => {
        const node = tr.doc.nodeAt(b.position);
        if (node.attrs.rid !== b.rid) tr.setNodeMarkup(b.position, undefined, { ...node.attrs, rid: b.rid });
      });
      if (tr.docChanged) editor.view.dispatch(tr);
    } finally { syncing = false; }
    projection = next;
    commitSource(next.source, "rich", beforeRichSelection);
    syncCM(); highlightLinkedSelection(); refresh();
  },
});

const fromCore = Annotation.define();
const decorateDiagnostics = StateEffect.define(), decorateSelection = StateEffect.define();
const decorations = StateField.define({
  create() { return { errors: Decoration.none, selection: Decoration.none }; },
  update(value, tr) {
    const next = { errors: value.errors.map(tr.changes), selection: value.selection.map(tr.changes) };
    for (const effect of tr.effects) {
      if (effect.is(decorateDiagnostics)) next.errors = effect.value;
      if (effect.is(decorateSelection)) next.selection = effect.value;
    }
    return next;
  },
  provide: field => [EditorView.decorations.from(field, value => value.errors), EditorView.decorations.from(field, value => value.selection)],
});
const syntax = ViewPlugin.fromClass(class {
  constructor(view) { this.decorations = this.paint(view); }
  update(update) { if (update.docChanged || update.viewportChanged) this.decorations = this.paint(update.view); }
  paint(view) {
    const spans = [];
    for (let n = 1; n <= view.state.doc.lines; n++) {
      const line = view.state.doc.line(n);
      if (/^=+ /u.test(line.text)) spans.push(Decoration.mark({ class: "syntax-heading" }).range(line.from, line.to));
      else {
        const re = /\/\/.*$|"(?:\\.|[^"\\])*"|[#@][!\w]*|[*_`$]/gu;
        for (const match of line.text.matchAll(re)) spans.push(Decoration.mark({ class: match[0].startsWith("//") ? "syntax-comment" : match[0][0] === '"' ? "syntax-string" : /^[#@]/u.test(match[0]) ? "syntax-code" : "syntax-marker" }).range(line.from + match.index, line.from + match.index + match[0].length));
      }
    }
    return Decoration.set(spans, true);
  }
}, { decorations: value => value.decorations });

cm = new EditorView({
  parent: $("#source-editor"),
  state: EditorState.create({ doc: source(), extensions: [
    lineNumbers(), drawSelection(), highlightActiveLine(), EditorView.lineWrapping, decorations, syntax,
    keymap.of([
      { key: "Mod-z", run: () => { queueMicrotask(() => undo(false)); return true; } },
      { key: "Mod-Shift-z", run: () => { queueMicrotask(() => undo(true)); return true; } },
      { key: "Mod-y", run: () => { queueMicrotask(() => undo(true)); return true; } }, ...defaultKeymap,
    ]),
    EditorView.contentAttributes.of({ "aria-label": "Notist 源码编辑器", spellcheck: "false" }),
    EditorView.exceptionSink.of(error => { errors.push(String(error)); console.error(error); }),
    EditorView.domEventHandlers({
      focus() { activeView = "source"; },
      compositionstart() { compositionStart(); }, compositionend() { compositionEnd(); },
    }),
    EditorView.updateListener.of(update => {
      const range = update.state.selection.main, line = update.state.doc.lineAt(range.head);
      $("#cursor-status").textContent = `Ln ${line.number}, Col ${range.head - line.from + 1} · UTF-8`;
      if (!update.docChanged || update.transactions.every(tr => tr.annotation(fromCore))) return;
      const old = update.startState.selection.main;
      try {
        commitSource(update.state.doc.toString(), "source", { view: "source", anchor: old.anchor, head: old.head });
        syncRich();
      } catch (error) {
        queueMicrotask(() => { syncCM(); syncRich(); toast("文档已有其他修改，视图已重新同步。"); });
        if (error.code !== "stale_version") { errors.push(String(error)); console.error(error); }
      }
    }),
    EditorView.theme({
      "&": { backgroundColor: "transparent", color: "#71806c", fontSize: "12px" },
      ".cm-content": { padding: "8px 0", fontFamily: '"SFMono-Regular", Consolas, monospace', caretColor: "#35694e", lineHeight: "2.05" },
      ".cm-line": { padding: "0 18px 0 10px" },
      ".cm-gutters": { backgroundColor: "transparent", color: "#b8c0b0", border: "none", fontSize: "10px" },
      ".cm-lineNumbers .cm-gutterElement": { padding: "0 10px 0 12px", minWidth: "38px" },
      ".cm-activeLine": { backgroundColor: "#e7eddd44" },
      "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": { backgroundColor: "#dae7ce !important" },
      ".cm-cursor": { borderLeftColor: "#35694e" },
    }),
  ] }),
});

function syncCM() {
  const patch = sourcePatch(cm.state.doc.toString(), source());
  if (patch) cm.dispatch({ changes: patch, annotations: fromCore.of(true) });
  cmVersion = core.snapshot().version;
}
function syncRich(saved = selection("rich")) {
  projection = withPositions(project(source()));
  richVersion = core.snapshot().version;
  syncing = true;
  try {
    rich.commands.setContent(projection.doc, { emitUpdate: false });
    if (projection.blocks.some(b => b.offsets)) rich.commands.setTextSelection({ from: sourceToRich(projection, saved.anchor), to: sourceToRich(projection, saved.head) });
  } finally { syncing = false; }
  refresh();
}
function highlightLinkedSelection() {
  if (!cm) return;
  const range = selection("rich");
  const from = Math.min(range.anchor, range.head), to = Math.max(range.anchor, range.head);
  cm.dispatch({ effects: decorateSelection.of(from < to && to <= cm.state.doc.length ? Decoration.set([Decoration.mark({ class: "linked-selection" }).range(from, to)]) : Decoration.none) });
}
function undo(redo) {
  const before = selection();
  const metadata = { kind: "projection-selection", view: before.view }, positions = [before.anchor, before.head];
  const event = redo ? core.redo(metadata, positions) : core.undo(metadata, positions);
  if (!event) return;
  const saved = resolveSelection(event, before);
  syncCM(); syncRich(saved);
  if (saved.view === "source") {
    cm.dispatch({ selection: { anchor: Math.min(saved.anchor, source().length), head: Math.min(saved.head, source().length) }, scrollIntoView: true }); cm.focus();
  } else rich.commands.focus();
}
function revealSource(from, to = from) {
  setSplit(true);
  cm.dispatch({ selection: { anchor: Math.min(from, source().length), head: Math.min(to, source().length) }, scrollIntoView: true }); cm.focus();
}
function setSplit(split) {
  $("#workspace").classList.toggle("document-only", !split);
  $("#split-view").classList.toggle("active", split); $("#document-only").classList.toggle("active", !split);
  cm?.requestMeasure();
}
for (const [name, command] of [["bold", "toggleBold"], ["italic", "toggleItalic"]]) {
  $(`#${name}`).addEventListener("mousedown", event => event.preventDefault());
  $(`#${name}`).onclick = () => rich.chain().focus()[command]().run();
}
$("#heading").onmousedown = event => event.preventDefault();
$("#heading").onclick = () => rich.chain().focus().toggleHeading({ level: 2 }).run();
$("#undo").onclick = () => undo(false); $("#redo").onclick = () => undo(true);
$("#split-view").onclick = () => setSplit(true); $("#document-only").onclick = () => setSplit(false);
$("#download").onclick = () => {
  const url = URL.createObjectURL(new Blob([source()], { type: "text/plain;charset=utf-8" }));
  const a = document.createElement("a"); a.href = url; a.download = "文字的两面.not"; a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
};
$("#diagnostic-status").onclick = () => { if (diagnostics.length) $("#diagnostics").hidden = !$("#diagnostics").hidden; };

let worker, analysisTimer, deadline;
function scheduleAnalysis() {
  clearTimeout(analysisTimer);
  languageReady = false;
  $("#diagnostic-status").textContent = "◌ 分析中…";
  analysisTimer = setTimeout(() => {
    if (!worker) {
      worker = new Worker("/language-worker.js", { type: "module" });
      worker.onmessage = ({ data }) => {
        if (data.revision !== revision) return;
        clearTimeout(deadline);
        if (data.error) { languageFailure(data.error); return; }
        languageReady = true;
        const syntaxErrors = (data.snapshot.syntax?.errors || []).map(d => ({ ...d, code: "syntax" }));
        diagnostics = [...new Map([...syntaxErrors, ...(data.snapshot.result?.diagnostics || [])].map(d => [`${d.source_id}:${d.range}:${d.message}`, d])).values()];
        renderDiagnostics();
      };
      worker.onerror = error => { languageFailure(error.message); };
    }
    clearTimeout(deadline);
    worker.postMessage({ revision, source: source() });
    deadline = setTimeout(() => languageFailure("语言分析超时"), 4000);
  }, 200);
}
function languageFailure(message) {
  worker?.terminate(); worker = null; clearTimeout(deadline);
  $("#diagnostic-status").textContent = "语言核暂不可用";
  $("#diagnostic-status").title = message;
  errors.push(message);
}
function renderDiagnostics() {
  const panel = $("#diagnostics"); panel.replaceChildren();
  const text = source(), spans = [];
  for (const diagnostic of diagnostics) {
    const [start, end] = diagnostic.range || [0, 0];
    const from = byteToUTF16(text, start), to = byteToUTF16(text, end);
    const button = document.createElement("button"); button.textContent = `${diagnostic.code}: ${diagnostic.message}`;
    button.onclick = () => revealSource(from, to); panel.append(button);
    if (from < to && to <= text.length) spans.push(Decoration.mark({ class: "diagnostic-mark" }).range(from, to));
  }
  cm.dispatch({ effects: decorateDiagnostics.of(Decoration.set(spans, true)) });
  $("#diagnostic-status").textContent = diagnostics.length ? `△ ${diagnostics.length} 条诊断 · 展开` : "✓ 语言核检查通过";
  $("#diagnostic-status").classList.toggle("error", !!diagnostics.length);
  if (!diagnostics.length) panel.hidden = true;
}

// Explicit test surface. Network mode is opt-in on the separate sync server.
window.probe = {
  rich, cm, syncSession, get core() { return core; },
  makeReplica: () => EditorCore.restore(EditorDocument, core.exportSnapshot()),
  inspect: () => ({ source: source(), cm: cm.state.doc.toString(), doc: rich.getJSON(), projection: projection.source, lastPatch, revision, diagnostics, languageReady, errors: [...errors], canUndo: core.undoState.can_undo, canRedo: core.undoState.can_redo }),
  reset(text = SAMPLE) {
    if (syncSession) throw new Error("Use a fresh room to reset a collaborative document");
    $("#toast").hidden = true;
    core.dispose(); core = createCore(text); observeCore();
    syncCM(); syncRich({ anchor: 0, head: 0 }); changed();
  },
  selectRich(from, to = from) { rich.commands.setTextSelection({ from: sourceToRich(projection, from), to: sourceToRich(projection, to) }); rich.commands.focus(); },
  undo, revealSource,
};
syncSession?.mount();
refresh(); scheduleAnalysis();
