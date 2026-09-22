import { EditorState, EditorSelection } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { defaultKeymap } from "@codemirror/commands";
import { LoroDoc, UndoManager, LORO_VERSION } from "loro-crdt";
import { LoroExtensions, undo, redo } from "loro-codemirror";

let editors = [];
const pause = () => new Promise((resolve) => setTimeout(resolve, 30));
const errors = [];
function create(id, doc, initialState) {
  const manager = new UndoManager(doc, { mergeInterval: 0 });
  const composition = [];
  const view = new EditorView({
    state: EditorState.create({
      doc: initialState,
      extensions: [
        EditorState.allowMultipleSelections.of(true),
        keymap.of(defaultKeymap),
        EditorView.exceptionSink.of((error) => errors.push(String(error))),
        EditorView.domEventHandlers({
          compositionstart: (e) => { composition.push({ type: e.type, data: e.data, trusted: e.isTrusted }); },
          compositionupdate: (e) => { composition.push({ type: e.type, data: e.data, trusted: e.isTrusted }); },
          compositionend: (e) => { composition.push({ type: e.type, data: e.data, trusted: e.isTrusted }); },
        }),
        LoroExtensions(doc, undefined, manager, (d) => d.getText("text")),
      ],
    }),
    parent: document.getElementById(id),
  });
  return { doc, manager, view, composition };
}
function inspect() {
  return editors.map(({ doc, manager, view, composition }) => ({
    text: view.state.doc.toString(), crdt: doc.getText("text").toString(),
    ranges: view.state.selection.ranges.map(({ anchor, head }) => ({ anchor, head })),
    mainIndex: view.state.selection.mainIndex,
    canUndo: manager.canUndo(), canRedo: manager.canRedo(),
    composing: view.composing, composition: [...composition],
  }));
}
async function reset(initial = "", equalState = false, mergeInterval = 0, settleLayout = true) {
  for (const e of editors) e.view.destroy();
  errors.length = 0;
  document.getElementById("a").replaceChildren();
  document.getElementById("b").replaceChildren();
  const a = new LoroDoc(); a.setPeerId(1);
  if (initial) a.getText("text").insert(0, initial);
  a.commit();
  const b = new LoroDoc(); b.setPeerId(2); b.import(a.export({ mode: "snapshot" }));
  editors = [create("a", a, equalState ? initial : ""), create("b", b, equalState ? initial : "")];
  for (const e of editors) e.manager.setMergeInterval(mergeInterval);
  if (settleLayout) await pause();
  else await Promise.resolve();
  return inspect();
}
async function synchronize() {
  const [a, b] = editors.map((e) => e.doc);
  a.import(b.export({ mode: "update", from: a.version() }));
  b.import(a.export({ mode: "update", from: b.version() }));
  await pause(); return inspect();
}
window.probe = {
  reset, inspect, errors, synchronize, version: LORO_VERSION(),
  async initialTransaction() {
    // Run after the binding's initialization microtask, before a layout-only
    // update can accidentally clear its isInitDispatch flag.
    await reset("abc", true, 0, false);
    editors[0].view.dispatch({ changes: { from: 3, insert: "中" } });
    await pause(); return inspect();
  },
  async select(peer, ranges, mainIndex = 0) {
    editors[peer].view.dispatch({ selection: EditorSelection.create(ranges.map(([a, h]) => EditorSelection.range(a, h)), mainIndex) });
    await pause(); return inspect();
  },
  async replace(peer, text) {
    const { view } = editors[peer]; view.dispatch(view.state.replaceSelection(text));
    await pause(); return inspect();
  },
  async edit(peer, from, to, insert) {
    editors[peer].view.dispatch({ changes: { from, to, insert } });
    await pause(); return inspect();
  },
  async undo(peer) { undo(editors[peer].view); await pause(); return inspect(); },
  async redo(peer) { redo(editors[peer].view); await pause(); return inspect(); },
  // Control experiment: avoid the official command's nested EditorView.dispatch.
  async directUndo(peer) { editors[peer].manager.undo(); await pause(); return inspect(); },
  async directRedo(peer) { editors[peer].manager.redo(); await pause(); return inspect(); },
  async remoteInsert(peer, at, string) {
    const d = editors[peer].doc, remote = new LoroDoc(); remote.setPeerId(77);
    remote.import(d.export({ mode: "snapshot" })); remote.getText("text").insert(at, string); remote.commit();
    d.import(remote.export({ mode: "update", from: d.version() }));
    await pause(); return inspect();
  },
  async directLocalInsert(peer, at, string) {
    const d = editors[peer].doc; d.getText("text").insert(at, string); d.commit();
    await pause(); return inspect();
  },
  async mixedRemote(peer, textFirst) {
    const d = editors[peer].doc, remote = new LoroDoc(); remote.setPeerId(78);
    remote.import(d.export({ mode: "snapshot" }));
    const setText = () => remote.getText("text").insert(0, "R");
    const setMeta = () => remote.getMap("metadata").set("title", "changed");
    if (textFirst) { setText(); setMeta(); } else { setMeta(); setText(); }
    remote.commit(); d.import(remote.export({ mode: "update", from: d.version() }));
    await pause(); return inspect();
  },
};
document.getElementById("sync").onclick = synchronize;
document.getElementById("undo").onclick = () => window.probe.undo(0);
document.getElementById("redo").onclick = () => window.probe.redo(0);
await reset("= Loro 验证\n\n在这里输入中文与 emoji 😀\n");
setInterval(() => { document.getElementById("state").textContent = JSON.stringify(inspect(), null, 2); }, 250);
window.ready = true;
