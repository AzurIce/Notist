import assert from "node:assert/strict";
import { mkdirSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { LoroDoc, Cursor, UndoManager } from "loro-crdt";

const results = [];
function check(name, fn) {
  try {
    const details = fn();
    results.push({ name, pass: true, details });
    console.log(`${details?.knownIssue ? "LIMIT" : "PASS"} ${name}`);
  } catch (error) {
    results.push({ name, pass: false, error: String(error), stack: error.stack });
    console.error(`FAIL ${name}: ${error}`);
  }
}
function doc(peer, initial = "") {
  const value = new LoroDoc();
  value.setPeerId(peer);
  if (initial) value.getText("text").insert(0, initial);
  value.commit();
  return value;
}
const text = (value) => value.getText("text");
const value = (d) => text(d).toString();
const snapshot = (d) => d.export({ mode: "snapshot" });
function clone(d, peer) {
  const out = doc(peer);
  out.import(snapshot(d));
  return out;
}
const sync = (a, b) => b.import(a.export({ mode: "update", from: b.version() }));
function attempt(fn) {
  try { return { ok: true, value: fn() ?? null }; }
  catch (e) { return { ok: false, error: String(e) }; }
}
function positions(string) {
  const out = [0];
  for (const char of string) out.push(out.at(-1) + char.length);
  return out;
}

check("32 seeds: three offline replicas, shuffled/duplicate updates, recovery", () => {
  let operationCount = 0;
  for (let seed = 1; seed <= 32; seed++) {
    let random = seed;
    const rand = (n) => {
      random = (Math.imul(random, 1664525) + 1013904223) >>> 0;
      return random % n;
    };
    const base = doc(99, "= 文档\nA😀B\né\n");
    const replicas = [1, 2, 3].map((id) => clone(base, id));
    const packets = [];
    for (const replica of replicas) {
      let expected = value(replica);
      for (let i = 0; i < 50; i++) {
        const before = replica.version();
        const boundaries = positions(expected);
        if (rand(3) === 0 && expected.length) {
          const i = rand(boundaries.length - 1);
          const from = boundaries[i], to = boundaries[i + 1];
          text(replica).delete(from, to - from);
          expected = expected.slice(0, from) + expected.slice(to);
        } else {
          const at = boundaries[rand(boundaries.length)];
          const insert = ["中", "😀", "é", "\n", "abc", "👩‍💻"][rand(6)];
          text(replica).insert(at, insert);
          expected = expected.slice(0, at) + insert + expected.slice(at);
        }
        replica.commit();
        assert.equal(value(replica), expected, `local edit oracle seed=${seed}`);
        packets.push(replica.export({ mode: "update", from: before }));
        operationCount++;
      }
    }
    for (let i = packets.length - 1; i > 0; i--) {
      const j = rand(i + 1);
      [packets[i], packets[j]] = [packets[j], packets[i]];
    }
    for (const [i, replica] of replicas.entries()) {
      for (const packet of i % 2 ? packets.toReversed() : packets) {
        replica.import(packet);
        replica.import(packet);
      }
    }
    assert.equal(value(replicas[0]), value(replicas[1]), `seed=${seed}`);
    assert.equal(value(replicas[1]), value(replicas[2]), `seed=${seed}`);
    assert.deepEqual(replicas[0].version().toJSON(), replicas[2].version().toJSON());
    const recovered = clone(base, 4);
    for (const packet of packets.toReversed()) recovered.import(packet);
    assert.equal(value(recovered), value(replicas[0]));
    assert.equal(value(clone(recovered, 5)), value(recovered));
  }
  return { seeds: 32, replicas: 3, operations: operationCount };
});

check("local undo/redo preserves remote insertion inside locally inserted text", () => {
  const a = doc(1), undo = new UndoManager(a, { mergeInterval: 0 });
  text(a).insert(0, "abc"); a.commit();
  const b = clone(a, 2);
  text(b).insert(1, "中😀"); b.commit(); sync(b, a);
  const merged = value(a);
  assert.equal(merged, "a中😀bc");
  assert.equal(undo.undo(), true);
  assert.equal(value(a), "中😀");
  assert.equal(undo.redo(), true);
  assert.equal(value(a), merged);
  sync(a, b);
  assert.equal(value(b), merged);
  return { merged, undone: "中😀" };
});

check("origin filtering and grouping", () => {
  const a = doc(1);
  const undo = new UndoManager(a, { mergeInterval: 0, excludeOriginPrefixes: ["sys:"] });
  text(a).insert(0, "base"); a.commit({ origin: "sys:load" });
  assert.equal(undo.canUndo(), false);
  text(a).insert(4, "A"); a.commit();
  text(a).insert(5, "B"); a.commit();
  undo.undo(); assert.equal(value(a), "baseA");
  undo.undo(); assert.equal(value(a), "base");
  assert.equal(undo.canUndo(), false);
  const b = doc(2);
  const grouped = new UndoManager(b, { mergeInterval: 60000 });
  text(b).insert(0, "A"); b.commit(); text(b).insert(1, "B"); b.commit();
  grouped.undo(); assert.equal(value(b), "");
});

check("changing peer clears existing undo history", () => {
  const a = doc(1), undo = new UndoManager(a, { mergeInterval: 0 });
  text(a).insert(0, "A"); a.commit();
  assert.equal(undo.canUndo(), true);
  a.setPeerId(2);
  assert.equal(undo.canUndo(), false);
  text(a).insert(1, "B"); a.commit();
  undo.undo(); assert.equal(value(a), "A");
  assert.equal(undo.canUndo(), false);
});

check("snapshot restores text/history but does not restore an UndoManager stack", () => {
  const a = doc(1), undo = new UndoManager(a, { mergeInterval: 0 });
  text(a).insert(0, "文😀"); a.commit();
  assert.equal(undo.canUndo(), true);
  const b = clone(a, 1), restoredUndo = new UndoManager(b, {});
  assert.equal(value(b), "文😀");
  assert.equal(restoredUndo.canUndo(), false);
});

check("cursor sides, remote insertion, deleted target, encoded cursor recovery", () => {
  const a = doc(1, "a😀bc"), b = clone(a, 2);
  const left = text(a).getCursor(3, -1), right = text(a).getCursor(3, 1);
  text(b).insert(3, "中"); b.commit(); sync(b, a);
  // Side is retained on the anchored element. It does not by itself implement
  // an editor's left/right insertion affinity at this boundary.
  assert.equal(a.getCursorPos(left).offset, 4);
  assert.equal(a.getCursorPos(right).offset, 4);
  const target = text(a).getCursor(4, 0);
  text(b).delete(4, 1); b.commit(); sync(b, a);
  const resolved = a.getCursorPos(target);
  assert.equal(resolved.offset, 4);
  assert.ok(resolved.update);
  const recovered = clone(a, 3);
  assert.equal(recovered.getCursorPos(Cursor.decode(target.encode())).offset, 4);
  assert.equal(recovered.getCursorPos(Cursor.decode(resolved.update.encode())).offset, 4);
  return { text: value(a), left: a.getCursorPos(left).offset, right: a.getCursorPos(right).offset, deleted: resolved.offset };
});

check("causally premature packet stays pending until predecessor arrives", () => {
  const a = doc(1), b = doc(2);
  text(a).insert(0, "A"); a.commit();
  const first = a.export({ mode: "update" }), before = a.version();
  text(a).insert(1, "B"); a.commit();
  const second = a.export({ mode: "update", from: before });
  const pending = b.import(second);
  assert.equal(value(b), "");
  b.import(second); b.import(first);
  assert.equal(value(b), "AB");
  return { pending: JSON.parse(JSON.stringify(pending, (_, v) => v instanceof Map ? [...v] : v)) };
});

check("full history accepts an old offline writer; trimming requires explicit recovery", () => {
  const a = doc(1, "abc"), offline = clone(a, 2);
  const oldVersion = offline.version();
  text(a).delete(1, 1); a.commit();
  text(a).insert(1, "NEW"); a.commit();
  text(offline).insert(2, "离线"); offline.commit();
  const packet = offline.export({ mode: "update", from: oldVersion });
  const full = clone(a, 3);
  full.import(packet);
  assert.ok(value(full).includes("离线"));
  const shallow = doc(4);
  shallow.import(a.export({ mode: "shallow-snapshot", frontiers: a.frontiers() }));
  assert.equal(value(shallow), value(a));
  const outcome = attempt(() => shallow.import(packet));
  // Characterize rejection, not an application-level recovery strategy.
  assert.equal(outcome.ok, false);
  assert.equal(value(shallow), value(a));
  const fresh = clone(shallow, 5);
  text(fresh).insert(text(fresh).length, "新"); fresh.commit();
  sync(fresh, shallow);
  assert.equal(value(shallow), value(a) + "新");
  return { fullMerged: value(full), oldOfflineImport: outcome };
});

check("known limitation: old deleted-target cursor panics after trimming; refreshed cursor works", () => {
  const a = doc(1, "abc");
  const cursor = text(a).getCursor(1, 0);
  text(a).delete(1, 1); a.commit();
  const resolved = a.getCursorPos(cursor);
  assert.ok(resolved.update);
  const shallow = doc(2);
  shallow.import(a.export({ mode: "shallow-snapshot", frontiers: a.frontiers() }));
  const updated = shallow.getCursorPos(Cursor.decode(resolved.update.encode()));
  assert.equal(updated.offset, 1);
  // Isolate a panic from the other semantic tests and preserve its full trace.
  const old = spawnSync(process.execPath, ["shallow-cursor.mjs"], { encoding: "utf8" });
  assert.notEqual(old.status, 0);
  assert.match(old.stderr, /panicked at/);
  mkdirSync("results", { recursive: true });
  writeFileSync("results/shallow-cursor.stderr", old.stderr);
  return { knownIssue: "Wasm panic on stale cursor after history trimming", exitCode: old.status, updated: { offset: updated.offset, side: updated.side } };
});

mkdirSync("results", { recursive: true });
writeFileSync("results/core.json", JSON.stringify({ loro: "1.16.2", runtime: process.version, results }, null, 2));
if (results.some((r) => !r.pass)) process.exitCode = 1;
