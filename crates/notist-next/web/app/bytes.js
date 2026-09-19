// UTF-8 字节偏移 → JS 字符串下标的映射工具。
// 快照里的 range 全是 UTF-8 字节区间 [start, end)，而 JS 字符串按 UTF-16 code unit
// 索引；两者在非 ASCII 文本上不一致，必须换算后再 slice。

// 缓存每个字符边界对应的累计 UTF-8 字节数，长度 = text.length + 1。
export function byteIndexCache(text) {
  const offsets = new Uint32Array(text.length + 1);
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    let width;
    if (code < 0x80) width = 1;
    else if (code < 0x800) width = 2;
    else if (code >= 0xd800 && code < 0xdc00) width = 4; // 代理对lead，整对计 4 字节
    else if (code >= 0xdc00 && code < 0xe000) width = 0; // trail，字节数记在 lead 上
    else width = 3;
    offsets[i + 1] = offsets[i] + width;
  }
  return offsets;
}

// 二分查找 byte 所在的字符边界下标；range 端点必然落在字符边界上。
export function charIndexForByte(offsets, byte) {
  let lo = 0;
  let hi = offsets.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (offsets[mid] < byte) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}
