import { inflateRawSync } from 'node:zlib';
/** Bounded parser for the trusted server's ZIP32 XLSX fixture, not production import. */
export function xml(bytes) {
  const data = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let at = 0,
    out = '';
  while (at + 30 <= bytes.length && data.getUint32(at, true) === 0x04034b50) {
    const method = data.getUint16(at + 8, true),
      size = data.getUint32(at + 18, true),
      name = data.getUint16(at + 26, true),
      extra = data.getUint16(at + 28, true),
      start = at + 30 + name + extra;
    const part = bytes.subarray(start, start + size);
    out += new TextDecoder().decode(method === 8 ? inflateRawSync(part) : part);
    at = start + size;
  }
  return out;
}
