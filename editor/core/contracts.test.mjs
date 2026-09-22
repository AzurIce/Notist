import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { mkdir, writeFile } from "node:fs/promises";
import { EditorCore, CoreError } from "./index.mjs";

const require = createRequire(import.meta.url);
const { EditorDocument } = require("../scripts/pkg-core-node/notist_editor_core_wasm.js");
const identity = { document_id: "test", history_id: "shared-history" };
const create = (writer, text = "", options = {}) => EditorCore.create(EditorDocument, { writer: String(writer), text, identity, ...options });
const edit = (core, edits, undoMetadata = null) => core.transact({ expectedVersion: core.snapshot().version, edits, undoMetadata, origin: "test" });
const native = request => {
  const result = spawnSync(fileURLToPath(new URL("../target/debug/examples/replay", import.meta.url)), { input: JSON.stringify(request), encoding: "utf8", maxBuffer: 32 * 1024 * 1024 });
  assert.equal(result.status, 0, result.stderr || String(result.error));
  return JSON.parse(result.stdout);
};

test("Wasm rejects invalid UTF-16 and stale writes without mutating the document", () => {
  const core = create(1, "A😀B");
  const snapshot = core.snapshot();
  for (const offset of [0.5, -1, NaN, Infinity, 4294967297]) {
    assert.throws(() => core.anchorAt(offset), error => error.code === "invalid_request");
  }
  assert.throws(() => edit(core, [{ from: 0, to: 1, insert: "X" }, { from: 2, to: 3, insert: "!" }]), error => error instanceof CoreError && error.code === "invalid_position");
  assert.deepEqual(core.snapshot(), snapshot);
  assert.equal(core.undoState.can_undo, false);
  edit(core, [{ from: 1, to: 3, insert: "中" }]);
  assert.throws(() => core.transact({ expectedVersion: snapshot.version, edits: [] }), error => error.code === "stale_version");
  assert.equal(core.snapshot().text, "A中B");
  assert.throws(() => { snapshot.version.clocks["1"] = 99; }, TypeError);
  core.dispose();
});

test("writer identities remain exact through JavaScript and constructors report structured errors", () => {
  const peer = "18446744073709551614";
  const core = create(peer, "中🧠");
  assert.equal(core.writerId, peer);
  assert.deepEqual(Object.keys(core.snapshot().version.clocks), [peer]);
  assert.throws(() => EditorCore.restore(EditorDocument, core.exportSnapshot(), { writer: peer }), error => error instanceof CoreError && error.code === "writer_already_used");
  assert.throws(() => create("1", "", { identity: { document_id: "", history_id: "x" } }), error => error instanceof CoreError && error.code === "invalid_identity");
  core.dispose();
});

test("subscriber delivery is deferred, ordered, isolated and unsubscribable", async () => {
  const failures = [];
  const core = create(1, "", { onListenerError: error => failures.push(error.message) });
  const seen = [];
  const stop = core.subscribe(event => seen.push(event));
  const stopBad = core.subscribe(() => { throw new Error("bad observer"); });
  const stopWriter = core.subscribe(() => edit(core, [{ from: 0, to: 0, insert: "bad" }]));
  edit(core, [{ from: 0, to: 0, insert: "A" }]);
  edit(core, [{ from: 1, to: 1, insert: "中" }]);
  assert.equal(core.snapshot().text, "A中");
  assert.deepEqual(seen, []); // still in the host transaction
  const late = [];
  core.subscribe(event => late.push(event));
  await Promise.resolve();
  assert.deepEqual(seen.map(e => e.after.text), ["A", "A中"]);
  assert.deepEqual(late, []);
  assert.equal(failures.length, 4);
  assert.ok(failures.some(error => error.includes("observer-initiated")));
  assert.equal(core.snapshot().text, "A中");
  stop(); stopBad(); stopWriter();
  edit(core, [{ from: 2, to: 2, insert: "B" }]); await Promise.resolve();
  assert.equal(seen.length, 2); assert.equal(late.length, 1);
  assert.ok(Object.isFrozen(seen[0].after));
  core.dispose(); assert.throws(() => core.snapshot(), /disposed/);
});

test("Wasm merge, anchors, grouped undo, and opaque selection metadata", () => {
  const a = create(1), selection = { ranges: [[0, 0], [0, 0]], main: 1 };
  edit(a, [{ from: 0, to: 0, insert: "abc" }], selection);
  const before = a.anchorAt(1, "before"), after = a.anchorAt(1, "after");
  const b = EditorCore.restore(EditorDocument, a.exportSnapshot(), { writer: "2" });
  edit(b, [{ from: 1, to: 1, insert: "中😀" }]);
  a.import(b.exportUpdatesSince(a.snapshot().version));
  assert.equal(a.resolveAnchor(before).offset, 1);
  assert.equal(a.resolveAnchor(after).offset, 4);
  const undone = a.undo({ selected: "abc" });
  assert.equal(a.snapshot().text, "中😀"); assert.deepEqual(undone.restored_metadata, selection);
  assert.deepEqual(a.redo().restored_metadata, { selected: "abc" });
  assert.equal(a.snapshot().text, "a中😀bc");
  a.beginUndoGroup();
  edit(a, [{ from: 0, to: 0, insert: "n" }]); edit(a, [{ from: 0, to: 1, insert: "你" }]);
  a.endUndoGroup(); a.undo(); assert.equal(a.snapshot().text, "a中😀bc");
  a.dispose(); b.dispose();
});

test("undo position lists survive replaced text, remote Unicode insertion and redo", () => {
  const a = create(1, "Hello world.");
  a.transact({ expectedVersion: a.snapshot().version, origin: "rich", edits: [{ from: 6, to: 11, insert: "*world*" }], undoMetadata: { main: 1 }, undoPositions: [6, 11, 0, 5] });
  const b = EditorCore.restore(EditorDocument, a.exportSnapshot(), { writer: "2" });
  edit(b, [{ from: 0, to: 0, insert: "远🧠" }]); a.import(b.exportUpdatesSince(a.snapshot().version));
  const undo = a.undo({ view: "source" }, [9, 16, 3, 8]);
  assert.deepEqual(undo.restored_positions, [9, 14, 0, 8]);
  assert.equal(undo.after.text, "远🧠Hello world.");
  const redo = a.redo();
  assert.deepEqual(redo.restored_positions, [9, 16, 3, 8]);
  a.dispose(); b.dispose();
});

test("native and Wasm replay identical edits, shuffled delivery, undo, anchors, and recovery", async () => {
  const initial = "= 文档\nA😀B\né\n";
  const base = EditorCore.create(EditorDocument, { identity: { document_id: "replay", history_id: "shared-history" }, writer: "99", text: initial });
  const docs = [1, 2, 3].map(peer => EditorCore.restore(EditorDocument, base.exportSnapshot(), { writer: String(peer) }));
  const packets = [], steps = [], trace = [], anchors = new Map();
  function step(action) {
    steps.push(action);
    const doc = docs[action.peer]; let observed = null;
    if (action.op === "edit") {
      const before = doc.snapshot().version;
      doc.transact({ expectedVersion: before, origin: "replay", edits: action.edits });
      packets.push(doc.exportUpdatesSince(before));
    } else if (action.op === "deliver") observed = doc.import(packets[action.packet], "replay").pending;
    else if (["undo", "redo"].includes(action.op)) {
      const before = doc.snapshot().version; observed = !!doc[action.op](); packets.push(doc.exportUpdatesSince(before));
    } else if (action.op === "anchor") anchors.set(action.name, doc.anchorAt(action.offset, action.affinity));
    else if (action.op === "resolve") observed = doc.resolveAnchor(anchors.get(action.name)).offset;
    else if (action.op === "begin") doc.beginUndoGroup();
    else if (action.op === "end") doc.endUndoGroup();
    trace.push({ snapshots: docs.map(d => d.snapshot()), undo: docs.map(d => d.undoState), observed });
  }
  let random = 17;
  const rand = n => { random = (Math.imul(random, 1664525) + 1013904223) >>> 0; return random % n; };
  for (let peer = 0; peer < 3; peer++) {
    step({ op: "anchor", peer, offset: 5, affinity: "before", name: `before-${peer}` });
    step({ op: "anchor", peer, offset: 5, affinity: "after", name: `after-${peer}` });
    for (let i = 0; i < 45; i++) {
      const source = docs[peer].snapshot().text, offsets = [0];
      for (const char of source) offsets.push(offsets.at(-1) + char.length);
      const index = rand(offsets.length);
      const from = offsets[index], to = i % 3 === 0 && index + 1 < offsets.length ? offsets[index + 1] : from;
      step({ op: "edit", peer, edits: [{ from, to, insert: ["中", "😀", "é", "\n", "abc", "👩‍💻"][rand(6)] }] });
    }
  }
  const delivery = packets.map((_, index) => index);
  for (let i = delivery.length - 1; i > 0; i--) { const j = rand(i + 1); [delivery[i], delivery[j]] = [delivery[j], delivery[i]]; }
  for (const peer of [0, 1, 2]) {
    for (const packet of peer % 2 ? delivery.toReversed() : delivery) {
      step({ op: "deliver", peer, packet });
      if (packet % 9 === 0) step({ op: "deliver", peer, packet });
    }
    step({ op: "resolve", peer, name: `before-${peer}` }); step({ op: "resolve", peer, name: `after-${peer}` });
  }
  for (const peer of [0, 1, 2]) { step({ op: "undo", peer }); step({ op: "redo", peer }); }
  const extra = packets.length - 6;
  for (const peer of [0, 1, 2]) for (let packet = extra; packet < packets.length; packet++) step({ op: "deliver", peer, packet });
  step({ op: "begin", peer: 0 });
  step({ op: "edit", peer: 0, edits: [{ from: 0, to: 0, insert: "zhong" }] });
  step({ op: "edit", peer: 0, edits: [{ from: 0, to: 5, insert: "中文" }] });
  step({ op: "end", peer: 0 }); step({ op: "undo", peer: 0 }); step({ op: "redo", peer: 0 });
  const result = native({ initial, steps });
  assert.deepEqual(result.trace, trace);
  const imported = EditorCore.restore(EditorDocument, result.packet, { writer: "900" });
  assert.equal(imported.snapshot().text, docs[0].snapshot().text);
  edit(imported, [{ from: 0, to: 0, insert: "来自 Wasm 🧠\n" }]);
  const restored = native({ restore: imported.exportSnapshot() });
  assert.equal(restored.imported.text, imported.snapshot().text);
  const returned = EditorCore.restore(EditorDocument, restored.packet, { writer: "902" });
  assert.equal(returned.snapshot().text, imported.snapshot().text + "\n来自 Rust 🦀");
  await mkdir(new URL("./results/", import.meta.url), { recursive: true });
  await writeFile(new URL("./results/interop.json", import.meta.url), JSON.stringify({ steps: steps.length, nativeWasmEqual: true, snapshotRoundTrip: true }, null, 2));
  console.log(`Native/Wasm: ${steps.length} identical steps; bidirectional snapshots passed`);
  base.dispose(); imported.dispose(); returned.dispose(); for (const doc of docs) doc.dispose();
});
