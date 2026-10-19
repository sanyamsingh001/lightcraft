// Bounded fixture-format checks only. They do not execute a browser or prove lifecycle behavior.
import assert from 'node:assert/strict';
import test from 'node:test';
import {inflateSync} from 'node:zlib';
import {crc32} from './backup-archive.mjs';
import {pressurePng, installPendingImportControl} from './original-read-races.mjs';
import {runInNewContext} from 'node:vm';

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

// Execute the serialized controls with explicit native-frame/IDB facades. These checks only
// validate the fixture's scheduling and transaction assumptions; browser evidence is pending.
function pendingControls() {
  const native = new Map(); let next = 100;
  class IDBObjectStore {
    constructor(name = 'files') {this.name = name; this.transaction = new EventTarget();}
    put(value, key) {return {value, key};}
  }
  const env = {IDBObjectStore,
    requestAnimationFrame: callback => {const id = next++; native.set(id, callback); return id;},
    cancelAnimationFrame: id => native.delete(id),
  };
  runInNewContext(`(${installPendingImportControl.toString()})()`, env);
  const tick = () => {
    const frames = [...native]; native.clear();
    for (const [, callback] of frames) callback(123);
  };
  return {env, native, tick, store: new IDBObjectStore()};
}

test('pending-import controls hold scheduled frames, preserve cancellation, and resume exactly once', () => {
  const {env, native, tick} = pendingControls(), calls = [];
  env.requestAnimationFrame(time => calls.push(time));
  assert.equal(native.size, 1);
  env.pendingImportControl.hold(); assert.equal(native.size, 0);
  const canceled = env.requestAnimationFrame(() => calls.push('canceled'));
  env.cancelAnimationFrame(canceled);
  env.requestAnimationFrame(() => calls.push('held'));
  tick(); assert.deepEqual(calls, []);
  env.pendingImportControl.resume(); env.pendingImportControl.resume();
  assert.equal(native.size, 2, 'repeated resume cannot duplicate native callbacks');
  tick(); assert.deepEqual(calls, [123, 'held']);
  tick(); assert.equal(calls.length, 2);
});

test('pending-import controls count committed original writes, without treating request creation as durable', () => {
  const {env, store} = pendingControls(), control = env.pendingImportControl;
  store.put('before hold', 'originals/before');
  env.pendingImportControl.hold();
  const request = store.put('bytes', 'originals/owned');
  store.put('catalog', 'library/catalog.log');
  assert.equal(request.key, 'originals/owned');
  assert.equal(control.stored.size, 0);
  store.transaction.dispatchEvent(new Event('complete'));
  assert.deepEqual([...control.stored], ['originals/owned']);
  const aborted = new env.IDBObjectStore();
  aborted.put('bytes', 'originals/aborted'); aborted.transaction.dispatchEvent(new Event('abort'));
  assert.deepEqual([...control.aborted], ['originals/aborted']);
  assert.equal(control.stored.has('originals/aborted'), false);
  control.resume();
  const later = new env.IDBObjectStore(); later.put('outside control', 'originals/later');
  later.transaction.dispatchEvent(new Event('complete'));
  assert.equal(control.stored.size, 1);
});
