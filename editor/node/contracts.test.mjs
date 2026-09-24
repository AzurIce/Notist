import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { EditorNode, MemoryStore } from "./index.mjs";
import { EditorDocument } from "../document/index.mjs";
const { NodeBinding, DocumentBinding } = createRequire(import.meta.url)("../scripts/pkg-editor-node/notist_editor_node_wasm.js");
const identity = { document_id: "test", history_id: "history" }, credential = "document-credential-for-testing";
function append(doc, text) { const before = doc.snapshot(); doc.transact({ expectedVersion: before.version, edits: [{ from: before.text.length, to: before.text.length, insert: text }] }); }
function node(store = new MemoryStore()) { return new EditorNode({ NodeBinding, DocumentBinding, store, onError() {} }); }

test("managed imports preserve pending operations through journal recovery and preserve mutation guards", async () => {
  const source = EditorDocument.create(DocumentBinding, { identity, text: "Hello" });
  const seed = source.exportSnapshot(), initial = source.snapshot().version;
  append(source, " A"); const middle = source.snapshot().version, a = source.exportUpdatesSince(initial);
  append(source, " B"); const b = source.exportUpdatesSince(middle);
  const store = new MemoryStore(), first = node(store), doc = await first.openDocument({identity,credential,seed});
  assert.equal(doc.import(b).pending, true); await first.flush();
  assert.equal(first.durableVersion("test"), null);
  await first.close();
  const second = node(store), restored = await second.openDocument({identity:{history_id:identity.history_id,document_id:identity.document_id},credential});
  let guarded = false;
  restored.subscribe(() => { try { restored.import(a); } catch (error) { guarded = /notification delivery/.test(error.message); } });
  const imported = restored.import(a); assert.equal(Object.isFrozen(imported), true);
  await second.flush(); assert.equal(restored.snapshot().text, "Hello A B"); assert.equal(guarded,true);
  await second.close(); source.dispose();
});

test("failed storage never reports durable and releases its host resource on close", async () => {
  let closed = false;
  const store = new MemoryStore(); store.append = async () => { throw new Error("disk full"); }; store.close = async () => { closed = true; };
  const failed = node(store);
  await assert.rejects(failed.openDocument({identity,credential,text:"unsaved"}), /disk full/);
  assert.equal(failed.durableVersion("test"), null);
  await assert.rejects(failed.close(), /disk full/); assert.equal(closed,true);
});
