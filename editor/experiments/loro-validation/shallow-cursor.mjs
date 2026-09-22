// Minimal isolated reproduction. An old deleted-target cursor must not be used
// after its history is trimmed. Currently this triggers a Wasm panic.
const { LoroDoc } = await import(process.env.PROBE_LATEST ? "loro-crdt-latest" : "loro-crdt");
const doc = new LoroDoc();
doc.getText("text").insert(0, "abc"); doc.commit();
const cursor = doc.getText("text").getCursor(1, 0);
doc.getText("text").delete(1, 1); doc.commit();
const shallow = new LoroDoc();
shallow.import(doc.export({ mode: "shallow-snapshot", frontiers: doc.frontiers() }));
console.log(shallow.getCursorPos(cursor));
