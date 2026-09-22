import assert from "node:assert/strict";
import { mkdirSync, writeFileSync, readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { LoroDoc, UndoManager } from "loro-crdt";

mkdirSync("results", { recursive: true });
const binary = process.env.LORO_PROBE_BINARY ?? "./target/debug/notist-loro-validation";
const docs = [1, 2, 3].map((id) => { const d = new LoroDoc(); d.setPeerId(id); return d; });
const packets = [], plan = [], trace = [], undos = new Map(), cursors = new Map();
function step(s) {
  plan.push(s);
  const d = docs[s.peer], t = d.getText("text");
  let observed = null;
  if (s.op === "edit") {
    const before = d.version();
    if (s.delete) t.delete(s.at, s.delete);
    if (s.insert) t.insert(s.at, s.insert);
    d.commit(); packets.push(d.export({ mode: "update", from: before }));
  } else if (s.op === "deliver") d.import(packets[s.packet]);
  else if (s.op === "track") undos.set(s.peer, new UndoManager(d, { mergeInterval: 0 }));
  else if (s.op === "undo" || s.op === "redo") {
    observed = undos.get(s.peer)[s.op]();
    packets.push(d.export({ mode: "update" }));
  } else if (s.op === "cursor") cursors.set(s.name, t.getCursor(s.at, 0));
  else if (s.op === "resolve") {
    const r = d.getCursorPos(cursors.get(s.name));
    observed = { offset: r.offset, side: r.side };
  }
  trace.push({ texts: docs.map((d) => d.getText("text").toString()), observed });
}
const edit = (peer, at, insert, del = 0) => step({ op: "edit", peer, at, insert, delete: del });
const deliver = (peer, packet) => step({ op: "deliver", peer, packet });
step({ op: "track", peer: 0 });
edit(0, 0, "A😀B\n中文");
deliver(1, 0); deliver(2, 0);
step({ op: "cursor", peer: 0, at: 3, name: "after-emoji" });
edit(1, 3, "👩‍💻"); deliver(0, 1);
step({ op: "resolve", peer: 0, name: "after-emoji" });
step({ op: "undo", peer: 0 });
step({ op: "redo", peer: 0 });
deliver(1, 3); deliver(2, 3);
step({ op: "resolve", peer: 0, name: "after-emoji" });

let seed = 123456789;
const rand = (n) => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed % n; };
const randomPackets = [];
for (let peer = 0; peer < 3; peer++) {
  for (let i = 0; i < 50; i++) {
    const string = docs[peer].getText("text").toString();
    const points = [0];
    for (const c of string) points.push(points.at(-1) + c.length);
    const point = rand(points.length);
    const del = point < points.length - 1 && rand(3) === 0 ? points[point + 1] - points[point] : 0;
    edit(peer, points[point], ["x", "汉", "😀", "é", "\n"][rand(5)], del);
    randomPackets.push(packets.length - 1);
  }
}
for (let i = randomPackets.length - 1; i > 0; i--) {
  const j = rand(i + 1); [randomPackets[i], randomPackets[j]] = [randomPackets[j], randomPackets[i]];
}
for (let peer = 0; peer < 3; peer++) {
  for (const packet of peer % 2 ? randomPackets.toReversed() : randomPackets) {
    deliver(peer, packet); deliver(peer, packet);
  }
}
assert.equal(docs[0].getText("text").toString(), docs[1].getText("text").toString());
assert.equal(docs[1].getText("text").toString(), docs[2].getText("text").toString());
writeFileSync("results/replay.json", JSON.stringify(plan));
const native = JSON.parse(execFileSync(binary, ["replay", "results/replay.json", "results/native.loro"], { encoding: "utf8" }));
assert.deepEqual(native.trace, trace, "native and Wasm must agree after EVERY step");
const fromNative = new LoroDoc(); fromNative.import(readFileSync("results/native.loro"));
assert.equal(fromNative.getText("text").toString(), docs[0].getText("text").toString());
writeFileSync("results/wasm.loro", docs[0].export({ mode: "snapshot" }));
const roundtrip = JSON.parse(execFileSync(binary, ["import", "results/wasm.loro", "results/roundtrip.loro"], { encoding: "utf8" }));
assert.equal(roundtrip.imported, docs[0].getText("text").toString());
docs[0].import(readFileSync("results/roundtrip.loro"));
assert.equal(docs[0].getText("text").toString(), roundtrip.imported + "\n来自Rust🦀");
const result = { pass: true, loro: "1.16.2", comparedSteps: plan.length, edits: 152, binaryRoundtrip: true };
writeFileSync("results/interop.json", JSON.stringify(result, null, 2));
console.log(result);
