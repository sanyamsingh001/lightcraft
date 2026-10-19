// Read-only validation of stored ZIP entries produced by the application. No archive mutation.
import assert from 'node:assert/strict';
export function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
  }
  return (crc ^ 0xffffffff) >>> 0;
}
export function readBackup(bytes) {
  const requireRange = (start, size) => {
    assert.ok(Number.isSafeInteger(start) && Number.isSafeInteger(size) && start >= 0 && size >= 0 && start + size <= bytes.length, 'ZIP range must be present');
  };
  const u16 = offset => { requireRange(offset, 2); return bytes.readUInt16LE(offset); };
  const u32 = offset => { requireRange(offset, 4); return bytes.readUInt32LE(offset); };
  let end = -1;
  for (let offset = bytes.length - 22; offset >= Math.max(0, bytes.length - 65557); offset--) {
    if (u32(offset) === 0x06054b50 && offset + 22 + u16(offset + 20) === bytes.length) { end = offset; break; }
  }
  assert.ok(end >= 0, 'ZIP end record must exist');
  assert.equal(u16(end + 4), 0); assert.equal(u16(end + 6), 0);
  const count = u16(end + 10), size = u32(end + 12), start = u32(end + 16);
  assert.equal(u16(end + 8), count);
  requireRange(start, size);
  const entries = new Map();
  let offset = start;
  for (let i = 0; i < count; i++) {
    assert.equal(u32(offset), 0x02014b50);
    assert.equal(u16(offset + 10), 0, 'LightCraft archives use stored entries');
    const crc = u32(offset + 16), length = u32(offset + 24);
    assert.equal(u32(offset + 20), length);
    const nameLength = u16(offset + 28), extraLength = u16(offset + 30), commentLength = u16(offset + 32), local = u32(offset + 42);
    requireRange(offset + 46, nameLength);
    const name = bytes.subarray(offset + 46, offset + 46 + nameLength).toString('utf8');
    assert.equal(u32(local), 0x04034b50);
    assert.equal(u16(local + 8), 0);
    const localNameLength = u16(local + 26), localExtraLength = u16(local + 28);
    requireRange(local + 30, localNameLength);
    assert.equal(bytes.subarray(local + 30, local + 30 + localNameLength).toString('utf8'), name);
    const dataStart = local + 30 + localNameLength + localExtraLength;
    requireRange(dataStart, length);
    const data = bytes.subarray(dataStart, dataStart + length);
    assert.equal(crc32(data), crc, `CRC for ${name}`);
    assert.ok(!entries.has(name), `duplicate ZIP entry ${name}`);
    entries.set(name, data);
    offset += 46 + nameLength + extraLength + commentLength;
  }
  assert.equal(offset, start + size);
  return entries;
}
