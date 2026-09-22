import { test } from "node:test";
import assert from "node:assert/strict";
import { project, serialize, withPositions, sourcePatch, richToSource, sourceToRich, byteToUTF16 } from "./projection.mjs";

test("untouched source is byte-for-byte identical, including opaque syntax and gaps", () => {
  const samples = ["", "\n\n", "  \n= Heading\n\nText *bold* and _em_.\n \n\n// comment\n#let n = 42\n", "@(label: \"x\")\nHello\n\nWorld", "Hello \\*literal\\* 🧠", "= Heading\n\n*未完成", "abc_def", "One\nwrapped line\n\nTwo", "/* open\n\nnot a paragraph\n*/\n\nEnd", "```not\n\nsource\n\n```\n\nEnd", '#let x = (\n\n  "one",\n\n  "two",\n)\n\nEnd', "Start\n\n#let x = (\n\nunfinished"];
  for (const source of samples) {
    const p = project(source);
    assert.equal(serialize(p.doc, p).source, source, JSON.stringify(source));
  }
});

test("multiline source constructs never expose their inner paragraphs", () => {
  for (const raw of ["/* a\n\nb\n*/", "```not\n\na\n\nb\n```", '#let x = (\n\n"a",\n\n"b"\n)', "#let x = (\n\na\n\nb"]) {
    const p = project(raw);
    assert.equal(p.blocks.length, 1, raw);
    assert.equal(p.blocks[0].node.type, "sourceIsland");
  }
});

test("one rich edit preserves unrelated source exactly", () => {
  const source = "// comment\n#let n = 42\n\n\nHello world.\n \n@(label: \"foo\")\nUntouched\n";
  const p = project(source), doc = structuredClone(p.doc);
  doc.content[1].content = [{ type: "text", text: "Hello " }, { type: "text", text: "world", marks: [{ type: "bold" }] }, { type: "text", text: "." }];
  const next = serialize(doc, p);
  assert.equal(next.source, source.replace("world", "*world*"));
  assert.deepEqual(sourcePatch(source, next.source), { from: source.indexOf("world"), to: source.indexOf("world") + 5, insert: "*world*" });
});

test("splitting a block assigns a fresh id and leaves existing separator spelling intact", () => {
  const p = project("One two\n \n\nThree\n"), doc = structuredClone(p.doc);
  const first = doc.content[0]; first.content[0].text = "One";
  doc.content.splice(1, 0, { ...structuredClone(first), content: [{ type: "text", text: "two" }] });
  const result = serialize(doc, p);
  assert.equal(result.source, "One\n\ntwo\n \n\nThree\n");
  assert.notEqual(result.blocks[0].rid, result.blocks[1].rid);
  assert.equal(serialize(result.doc, result).source, result.source);
});

test("opaque blocks cannot be deleted or silently rewritten through rich operations", () => {
  const p = project("Hello\n\n#let n = 42");
  assert.throws(() => serialize({ type: "doc", content: [p.doc.content[0]] }, p));
  const changed = structuredClone(p.doc); changed.content[1].attrs.raw = "altered";
  assert.throws(() => serialize(changed, p));
});

test("new literal source syntax is escaped and invalid word-boundary styling is rejected", () => {
  const p = project("Hello"), doc = structuredClone(p.doc);
  doc.content[0].content[0].text = "Hello # world * [ ] 🧠";
  const result = serialize(doc, p);
  assert.equal(result.source, "Hello \\# world \\* \\[ \\] 🧠");
  doc.content[0].content = [{ type: "text", text: "a" }, { type: "text", text: "b", marks: [{ type: "bold" }] }, { type: "text", text: "c" }];
  assert.throws(() => serialize(doc, p));
});

test("source/ProseMirror mapping and source patch boundaries handle emoji and CJK", () => {
  const p = withPositions(project("= 中文 🧠\n\n有 *强调* 和 \\*。"));
  for (const text of ["中文", "🧠", "强调", "和"]) {
    const sourceOffset = p.source.indexOf(text);
    assert.equal(richToSource(p, sourceToRich(p, sourceOffset)), sourceOffset);
  }
  for (const [before, after] of [["x😀y", "x😁y"], ["🧠z", "az"], ["a", "a🧠"], ["中文", "中🧠文"]]) {
    const patch = sourcePatch(before, after);
    assert.equal(before.slice(0, patch.from) + patch.insert + before.slice(patch.to), after);
    assert.ok(!/[\uD800-\uDBFF]$/u.test(before.slice(0, patch.from)));
  }
  assert.equal(byteToUTF16("中🧠x", 7), 3);
});
