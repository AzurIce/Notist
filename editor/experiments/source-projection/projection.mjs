// Deliberately conservative editing projection, NOT the Notist parser.
// The language worker uses the real Rust language core for diagnostics.
let serial = 0;
const id = () => `block-${++serial}`;
const marker = { bold: "*", italic: "_" };
const reserved = /[\\*_#$@\[\]`=\/]/u;
const word = c => !!c && /[\p{L}\p{N}]/u.test(c) && !/[\u2e80-\u9fff\uac00-\ud7af]/u.test(c);

export function inlineSource(content = []) {
  let raw = "", active = [], pos = 0;
  const offsets = [0];
  for (const node of content) {
    if (node.type !== "text") throw new Error("此原型仅支持段落、标题、加粗和斜体。");
    const marks = (node.marks || []).map(m => m.type).sort();
    if (marks.some(m => !marker[m])) throw new Error("这个格式尚未实现源码映射。");
    let shared = 0;
    while (shared < active.length && active[shared] === marks[shared]) shared++;
    raw += active.slice(shared).reverse().map(m => marker[m]).join("");
    raw += marks.slice(shared).map(m => marker[m]).join("");
    active = marks;
    offsets[pos] = raw.length;
    for (const char of node.text) {
      if (reserved.test(char)) raw += "\\";
      offsets[pos] = raw.length;
      raw += char;
      // ProseMirror, CodeMirror and Loro's JS API all use UTF-16 offsets.
      if (char.length === 2) offsets[++pos] = raw.length - 1;
      offsets[++pos] = raw.length;
    }
  }
  raw += active.reverse().map(m => marker[m]).join("");
  return { raw, offsets };
}

function parseInline(raw) {
  const content = [], marks = [];
  const append = text => {
    const types = [...marks].sort();
    const last = content.at(-1);
    if (last && JSON.stringify(last.marks || []) === JSON.stringify(types.map(type => ({ type })))) last.text += text;
    else content.push({ type: "text", text, ...(types.length ? { marks: types.map(type => ({ type })) } : {}) });
  };
  for (let i = 0; i < raw.length;) {
    const char = String.fromCodePoint(raw.codePointAt(i));
    if (char === "\\") {
      const next = raw[i + 1];
      if (!next || !reserved.test(next)) return null;
      append(next); i += 2; continue;
    }
    if (char === "*" || char === "_") {
      const type = char === "*" ? "bold" : "italic";
      if (marks.at(-1) === type) marks.pop();
      else {
        // Notist treats a marker between Latin word characters literally.
        if (marks.includes(type) || (word([...raw.slice(0, i)].at(-1)) && word(raw[i + 1]))) return null;
        marks.push(type);
      }
      i++; continue;
    }
    if (reserved.test(char) || /[\r\n]/u.test(char)) return null;
    append(char); i += char.length;
  }
  if (marks.length) return null;
  // Never silently normalize an imported spelling or mark arrangement.
  if (inlineSource(content).raw !== raw) return null;
  return content;
}

function editable(raw, rid) {
  const heading = /^(={1,3}) (.*)$/u.exec(raw);
  const body = heading ? heading[2] : raw;
  if (!heading && /^(?:\s|[-+/]\s|\d+\.\s)/u.test(raw)) return null;
  const content = parseInline(body);
  if (!content) return null;
  return { type: heading ? "heading" : "paragraph", attrs: { rid, ...(heading ? { level: heading[1].length } : {}) }, ...(content.length ? { content } : {}) };
}

// Blank lines split blocks only outside comments, raw spans, math, and bracketed
// expressions. An unfinished construct conservatively protects the remainder.
// False negatives (opaque blocks) are preferable to editing expression interiors.
function separators(source) {
  const result = [];
  let comment = 0, ticks = 0, math = false, quote = null, brackets = [];
  let lineComment = false, code = false;
  for (let i = 0; i < source.length; i++) {
    const c = source[i], pair = source.slice(i, i + 2);
    if (lineComment) { if (c !== "\n") continue; lineComment = false; }
    if (comment) {
      if (pair === "/*") { comment++; i++; }
      else if (pair === "*/") { comment--; i++; }
      continue;
    }
    if (ticks) {
      if (c === "`") {
        const run = /^`+/u.exec(source.slice(i))[0].length;
        if (run === ticks) ticks = 0;
        i += run - 1;
      }
      continue;
    }
    if (c === "\\") { i++; continue; }
    if (quote) { if (c === quote) quote = null; continue; }
    if (math) { if (c === "$") math = false; continue; }
    if (pair === "//") { lineComment = true; i++; continue; }
    if (pair === "/*") { comment = 1; i++; continue; }
    if (c === "`") { ticks = /^`+/u.exec(source.slice(i))[0].length; i += ticks - 1; continue; }
    if (c === "$") { math = true; continue; }
    if (c === "#" || c === "@") code = true;
    if (code && c === '"') { quote = c; continue; }
    if ((code && "({".includes(c)) || c === "[") brackets.push(c);
    if (")}]".includes(c) && brackets.length) brackets.pop();
    if (c === "\n" && !brackets.length) {
      code = false;
      const gap = /^\n[ \t]*\n(?:[ \t]*\n)*/u.exec(source.slice(i));
      if (gap) { result.push([i, i + gap[0].length]); i += gap[0].length - 1; }
    }
  }
  return result;
}

export function project(source) {
  const prefix = /^[ \t\r\n]*/u.exec(source)[0];
  const suffix = source.length === prefix.length ? "" : /[ \t\r\n]*$/u.exec(source)[0];
  const body = source.slice(prefix.length, source.length - suffix.length);
  const blocks = [], content = [];
  let start = 0, gap = "";
  for (const [end, next] of [...separators(body), [body.length, body.length]]) {
    const raw = body.slice(start, end), rid = id();
    const node = editable(raw, rid) || { type: "sourceIsland", attrs: { rid, raw } };
    content.push(node);
    blocks.push({ rid, raw, gap, from: prefix.length + start, to: prefix.length + end, node });
    gap = body.slice(end, next); start = next;
  }
  return { source, prefix, suffix, blocks, doc: { type: "doc", content } };
}

export function serialize(doc, previous) {
  const originals = new Map(previous.blocks.map(b => [b.rid, b]));
  const protectedBefore = previous.blocks.filter(b => b.node.type === "sourceIsland").map(b => b.rid);
  const protectedAfter = (doc.content || []).filter(n => n.type === "sourceIsland").map(n => n.attrs.rid);
  if (JSON.stringify(protectedBefore) !== JSON.stringify(protectedAfter)) {
    throw new Error("源码块需要在右侧修改；文档操作不能删除或移动它。");
  }
  let source = previous.prefix, position = 0;
  const blocks = [], content = [], seen = new Set();
  for (const input of doc.content || []) {
    const node = structuredClone(input);
    const original = originals.get(node.attrs?.rid);
    const rid = node.attrs?.rid && !seen.has(node.attrs.rid) ? node.attrs.rid : id();
    seen.add(rid);
    node.attrs = { ...node.attrs, rid };
    const gap = blocks.length ? (rid === original?.rid ? original.gap || "\n\n" : "\n\n") : "";
    source += gap;
    let raw, offsets, lead = "";
    if (node.type === "sourceIsland") {
      if (!original || node.attrs.raw !== original.raw) throw new Error("源码块需要在右侧修改。");
      raw = original.raw;
    } else {
      if (!["paragraph", "heading"].includes(node.type)) throw new Error("这个块还没有源码映射。");
      const inline = inlineSource(node.content);
      lead = node.type === "heading" ? "=".repeat(node.attrs.level) + " " : "";
      raw = lead + inline.raw;
      // Prevent e.g. bold inside a Latin word becoming literal source markers.
      if (raw && !editable(raw, rid)) throw new Error("这个位置无法无损转换。可先按词选择，或在右侧直接编辑源码。");
      offsets = inline.offsets.map(n => n + lead.length);
    }
    blocks.push({ rid, node, raw, gap, from: source.length, to: source.length + raw.length, position, offsets });
    source += raw;
    content.push(node);
    position += node.type === "sourceIsland" ? 1 : 2 + (node.content || []).reduce((n, t) => n + t.text.length, 0);
  }
  source += previous.suffix;
  return { source, blocks, prefix: previous.prefix, suffix: previous.suffix, doc: { type: "doc", content } };
}

export function withPositions(projection) { return serialize(projection.doc, projection); }

export function richToSource(projection, pos) {
  for (const b of projection.blocks) {
    if (!b.offsets) { if (pos === b.position) return b.from; continue; }
    const local = pos - b.position - 1;
    if (local >= 0 && local < b.offsets.length) return b.from + b.offsets[local];
  }
  return pos <= 0 ? 0 : projection.source.length;
}

export function sourceToRich(projection, offset) {
  let closest = 1, distance = Infinity;
  for (const b of projection.blocks) {
    if (!b.offsets) continue;
    for (let i = 0; i < b.offsets.length; i++) {
      const d = Math.abs(b.from + b.offsets[i] - offset);
      if (d < distance) { distance = d; closest = b.position + 1 + i; }
    }
  }
  return closest;
}

export function sourcePatch(before, after) {
  if (before === after) return null;
  let from = 0, a = before.length, b = after.length;
  while (from < a && from < b && before[from] === after[from]) from++;
  // Do not split a surrogate pair when two emoji share a leading surrogate.
  if (from && /[\uDC00-\uDFFF]/u.test(before[from] || after[from] || "")) from--;
  while (a > from && b > from && before[a - 1] === after[b - 1]) { a--; b--; }
  if (a < before.length && /[\uDC00-\uDFFF]/u.test(before[a])) { a++; b++; }
  return { from, to: a, insert: after.slice(from, b) };
}

export function byteToUTF16(source, offset) {
  let bytes = 0, chars = 0;
  for (const char of source) {
    if (bytes >= offset) break;
    bytes += new TextEncoder().encode(char).length;
    chars += char.length;
  }
  return chars;
}
