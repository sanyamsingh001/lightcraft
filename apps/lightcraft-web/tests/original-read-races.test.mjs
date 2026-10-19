// Bounded fixture-format checks only. They do not execute a browser or prove lifecycle behavior.
import assert from 'node:assert/strict';
import test from 'node:test';
import {inflateSync} from 'node:zlib';
import {crc32} from './backup-archive.mjs';
import {pressurePng} from './original-read-races.mjs';

test('generated race fixtures are distinct ordinary RGB PNGs with valid chunks and scanlines', () => {
  const fixtures = [pressurePng(0, 12, 8), pressurePng(1, 12, 8)];
  assert.notDeepEqual(fixtures[0], fixtures[1]);
  for (const [seed, bytes] of fixtures.entries()) {
    assert.deepEqual([...bytes.subarray(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10]);
    const chunks = new Map(); let offset = 8;
    while (offset < bytes.length) {
      const size = bytes.readUInt32BE(offset), type = bytes.subarray(offset + 4, offset + 8).toString('ascii');
      assert.ok(offset + size + 12 <= bytes.length);
      assert.equal(crc32(bytes.subarray(offset + 4, offset + 8 + size)), bytes.readUInt32BE(offset + size + 8));
      chunks.set(type, bytes.subarray(offset + 8, offset + 8 + size)); offset += size + 12;
    }
    assert.deepEqual([...chunks.keys()], ['IHDR', 'IDAT', 'IEND']);
    const header = chunks.get('IHDR');
    assert.equal(header.readUInt32BE(0), 12); assert.equal(header.readUInt32BE(4), 8);
    assert.deepEqual([...header.subarray(8)], [8, 2, 0, 0, 0]);
    const pixels = inflateSync(chunks.get('IDAT')), stride = 1 + 12 * 3;
    assert.equal(pixels.length, stride * 8);
    for (let y = 0; y < 8; y++) {
      assert.equal(pixels[y * stride], 0);
      for (let x = 0; x < 12; x++) {
        assert.deepEqual([...pixels.subarray(y * stride + 1 + x * 3, y * stride + 4 + x * 3)],
          [(x + seed) & 255, (y + seed * 3) & 255, (x + y + seed * 7) & 255]);
      }
    }
  }
});

test('CRC checker has a known control and fixture allocation inputs are bounded', () => {
  assert.equal(crc32(Buffer.from('123456789')), 0xcbf43926);
  for (const args of [[-1, 12, 8], [256, 12, 8], [1, 4097, 8], [1, 12, 2732], [1, 0, 8], [1, 12.5, 8]]) {
    assert.throws(() => pressurePng(...args));
  }
});
