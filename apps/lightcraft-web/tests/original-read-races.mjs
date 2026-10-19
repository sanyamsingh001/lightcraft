// Parent-run real-browser race controls, imported explicitly by browser-lifecycle.mjs.
// LIGHTCRAFT_ORIGINAL_READ_RACES=required LIGHTCRAFT_ORIGINAL_RACE_SCRATCH=<private scratch>
// Ordinary file pickers import valid RGB PNGs. About 800 MiB of originals exercise the
// unchanged production 768 MiB LRU; allow several GiB of browser memory and IDB quota.
// These fixtures build nothing and add no production budget overrides or test commands.
import assert from 'node:assert/strict';
import {mkdtemp, realpath, rm, writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {deflateSync} from 'node:zlib';
import {crc32} from './backup-archive.mjs';

// Original procedural RGB pixels, standard PNG scanlines/zlib/chunks, no ancillary padding.
export function pressurePng(seed, width = 4096, height = 2731) {
  assert.ok(Number.isInteger(seed) && seed >= 0 && seed < 256);
  assert.ok(Number.isInteger(width) && width > 0 && width <= 4096);
  assert.ok(Number.isInteger(height) && height > 0 && height <= 2731);
  const stride = 1 + width * 3;
  const pixels = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const offset = y * stride + 1 + x * 3;
      pixels[offset] = (x + seed) & 255;
      pixels[offset + 1] = (y + seed * 3) & 255;
      pixels[offset + 2] = (x + y + seed * 7) & 255;
    }
  }
  const chunk = (type, data) => {
    const result = Buffer.alloc(data.length + 12);
    result.writeUInt32BE(data.length); result.write(type, 4, 4, 'ascii'); data.copy(result, 8);
    result.writeUInt32BE(crc32(result.subarray(4, -4)), result.length - 4);
    return result;
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width); header.writeUInt32BE(height, 4); header[8] = 8; header[9] = 2;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', header),
    chunk('IDAT', deflateSync(pixels, {level: 0})), chunk('IEND', Buffer.alloc(0))]);
}

// Serialized into one new browser context. Freeze only animation frames, while file reads,
// IDB transactions and JS continuations proceed. No production cache or command overrides.
export function installPendingImportControl() {
  const nativeFrame = requestAnimationFrame.bind(globalThis), nativeCancel = cancelAnimationFrame.bind(globalThis);
  const frames = new Map(); let next = 1, held = false;
  const control = globalThis.pendingImportControl = {stored: new Set(), aborted: [],
    hold() {
      held = true;
      for (const frame of frames.values()) {
        if (frame.native !== null) nativeCancel(frame.native);
        frame.native = null;
      }
    },
    resume() {held = false; for (const [id, frame] of frames) schedule(id, frame);},
  };
  function schedule(id, frame) {
    if (frame.native !== null) return;
    frame.native = nativeFrame(timestamp => {
      frame.native = null;
      if (held) return;
      frames.delete(id); frame.callback(timestamp);
    });
  }
  globalThis.requestAnimationFrame = callback => {
    const id = next++, frame = {callback, native: null}; frames.set(id, frame);
    if (!held) schedule(id, frame);
    return id;
  };
  globalThis.cancelAnimationFrame = id => {
    const frame = frames.get(id);
    if (frame?.native !== null && frame?.native !== undefined) nativeCancel(frame.native);
    frames.delete(id);
  };
  const put = IDBObjectStore.prototype.put;
  IDBObjectStore.prototype.put = function(value, key) {
    const request = put.call(this, value, key);
    if (held && this.name === 'files' && typeof key === 'string' && key.startsWith('originals/')) {
      this.transaction.addEventListener('complete', () => control.stored.add(key), {once: true});
      this.transaction.addEventListener('abort', () => control.aborted.push(key), {once: true});
    }
    return request;
  };
}

async function pendingPickerBatch({browser, command, until, opened}, directory) {
  const paths = []; let accepted = 0;
  for (let seed = 0; seed < 25; seed++) {
    const file = join(directory, `pending-batch-${seed}.png`), bytes = pressurePng(seed);
    accepted += bytes.length; await writeFile(file, bytes); paths.push(file);
  }
  assert.ok(accepted > (768 << 20), 'batch must exceed the unchanged production cache');
  const context = await browser.newContext({acceptDownloads: true});
  try {
    await context.addInitScript(installPendingImportControl);
    const page = await opened(context, 'store=idb&workers=0');
    // Hide pressure photos from the grid to keep this assertion about ingress and original
    // storage, without gratuitous full-size renders during a 25-file import.
    await command(page, 'library.filter', {text: 'pending-batch-no-visible-photos'});
    await command(page, 'library.select', {ids: []});
    const chooser = page.waitForEvent('filechooser'); await command(page, 'file.addPhotos');
    const picker = await chooser;
    await page.evaluate(() => pendingImportControl.hold());
    try {
      await picker.setFiles(paths);
      await until(() => page.evaluate(() => pendingImportControl.stored.size === 25));
      // Backend::write awaits transaction completion. Two event-loop turns let its Rust
      // continuation queue every durable path, with all app logic frames still held.
      await page.evaluate(() => new Promise(resolve => setTimeout(() => setTimeout(resolve, 0), 0)));
      assert.deepEqual(await page.evaluate(() => pendingImportControl.aborted), []);
    } finally {await page.evaluate(() => pendingImportControl.resume());}
    await until(async () => {
      const s = await command(page, 'web.stats');
      // The old candidate has no pendingImports diagnostic; its short catalog fails below.
      return s.pendingImports === undefined || s.pendingImports === 0;
    });
    const photos = (await command(page, 'catalog.query', {filter: {}, limit: 200})).photos.filter(p => p.fileName.startsWith('pending-batch-'));
    assert.equal(photos.length, 25, 'every durable pick must reach the real catalog after cache eviction');
    const keys = new Set(await page.evaluate(() => [...pendingImportControl.stored]));
    for (const photo of photos) {
      const inspected = await command(page, 'photo.inspect', {id: photo.id});
      const hash = inspected.source.path.match(/^web\/([^/]+)\//)?.[1];
      assert.ok(hash && keys.has(`originals/${hash}`), 'catalog original must be one of the committed picker blobs');
    }
    const stats = await command(page, 'web.stats');
    assert.equal(stats.pendingImports, 0); assert.ok(stats.originalBytesInMemory <= (768 << 20));
    assert.doesNotMatch(await page.locator('#lightcraft_recovery_status').textContent(), /Could not import|Could not load/);
    console.log('PASS actual pending picker batch: 25 committed PNGs exceed LRU, all imported without increasing 768 MiB budget');
  } finally {
    await context.close();
    for (const file of paths) await rm(file);
  }
}

export async function runOriginalReadRaces({browser, base, command, until, opened, png, checkExport}) {
  assert.ok(process.env.LIGHTCRAFT_ORIGINAL_RACE_SCRATCH, 'provide an existing private fixture scratch directory');
  const directory = await mkdtemp(join(await realpath(process.env.LIGHTCRAFT_ORIGINAL_RACE_SCRATCH), 'original-read-races-'));
  try {
    await pendingPickerBatch({browser, command, until, opened}, directory);
    for (const [fallback, oldFailure, currentFailure] of [[false, false, false], [true, true, false], [false, false, true]]) {
      const context = await browser.newContext({acceptDownloads: true});
      try {
        await context.addInitScript(() => {
          globalThis.readRace = {requests: [], callbacks: [], writes: 0};
          const held = new WeakSet(), get = IDBObjectStore.prototype.get, put = IDBObjectStore.prototype.put;
          IDBObjectStore.prototype.get = function(key) {
            const request = get.call(this, key);
            if (this.name === 'files' && key === sessionStorage.getItem('read-race-key')) {
              readRace.requests.push(key); held.add(request);
            }
            return request;
          };
          IDBObjectStore.prototype.put = function(value, key) {
            if (this.name === 'files' && key === sessionStorage.getItem('read-race-key')) readRace.writes++;
            return put.call(this, value, key);
          };
          const success = Object.getOwnPropertyDescriptor(IDBRequest.prototype, 'onsuccess');
          if (!success?.set) throw new Error('fixture requires native request callbacks');
          Object.defineProperty(IDBRequest.prototype, 'onsuccess', {configurable: true, get: success.get, set(callback) {
            success.set.call(this, held.has(this) && callback ? function(event) {
              const request = this;
              readRace.callbacks.push({released: false, release(fail, message) {
                assertUnused(this.released); this.released = true;
                if (fail) {
                  Object.defineProperty(request, 'error', {configurable: true, value: new DOMException(message, 'NotReadableError')});
                  if (!request.onerror) throw new Error('fixture requires native error handler');
                  const error = new Error(message);
                  request.onerror.call(request, error);
                } else callback.call(request, event);
              }});
            } : callback);
          }});
          function assertUnused(released) { if (released) throw new Error('fixture callback already released'); }
        });
        if (fallback) await context.route('**/worker.js', async route => {
          const response = await route.fetch();
          await route.fulfill({response, body: `const open = IDBFactory.prototype.open;
            IDBFactory.prototype.open = function(name, ...args) {
              if (name === 'lightcraft') throw new DOMException('fixture worker storage refusal', 'UnknownError');
              return open.call(this, name, ...args);
            };\n` + await response.text()});
        });
        const page = await opened(context, 'store=idb&workers=0');
        const bytes = await png(page);
        async function pick(file) {
          const pending = page.waitForEvent('filechooser'); await command(page, 'file.addPhotos');
          await (await pending).setFiles(file);
        }
        async function photoNamed(name) {
          let photo;
          await until(async () => {
            photo = (await command(page, 'catalog.query', {filter: {}, limit: 200})).photos.find(p => p.fileName === name);
            return !!photo;
          });
          return photo;
        }
        await pick({name: 'read-race.png', mimeType: 'image/png', buffer: bytes});
        const photo = await photoNamed('read-race.png');
        await command(page, 'view.detail');
        await command(page, 'library.select', {ids: [photo.id], active: photo.id});
        await checkExport(page);
        const inspected = await command(page, 'photo.inspect', {id: photo.id});
        const hash = inspected.source.path.match(/^web\/([^/]+)\//)?.[1]; assert.ok(hash);
        await until(async () => {const s = await command(page, 'web.stats'); return !s.dirty && !s.flushing;});
        await page.evaluate(key => sessionStorage.setItem('read-race-key', key), `originals/${hash}`);
        const url = new URL(base); url.search = fallback ? 'store=idb&workers=2' : 'store=idb&workers=0';
        await page.goto(url.href); await page.waitForFunction(() => globalThis.lightcraft?.command);
        await page.locator('#lightcraft_loading').waitFor({state: 'hidden'});
        await command(page, 'library.select', {ids: [photo.id], active: photo.id});
        await until(() => page.evaluate(() => readRace.callbacks.length === 1));
        assert.equal((await command(page, 'web.stats')).originalsInMemory, 0);
        await pick({name: 'read-race.png', mimeType: 'image/png', buffer: bytes});
        await until(async () => (await command(page, 'web.stats')).originalBytesInMemory === bytes.length);
        assert.equal(await page.evaluate(() => readRace.writes), 1, 'same healthy original really reimported and stored');
        await command(page, 'library.select', {ids: []}); // prevent active-photo prefetch during pressure
        await command(page, 'view.detail');
        await command(page, 'library.filter', {text: 'read-race.png'});
        let accepted = 0;
        // Each distinct normal PNG is about 32 MiB. Do not change production cache budgets.
        for (let seed = 0; seed < 25; seed++) {
          const file = join(directory, `pressure-${seed}.png`), buffer = pressurePng(seed);
          accepted += buffer.length; await writeFile(file, buffer);
          await pick(file); await photoNamed(`pressure-${seed}.png`);
          await until(async () => {const s = await command(page, 'web.stats'); return !s.dirty && !s.flushing;});
          await rm(file);
        }
        assert.ok(accepted > (768 << 20));
        await until(async () => {const s = await command(page, 'web.stats'); return s.renderQueue === 0 && s.rendersInFlight === 0;});
        // Sources/rendered previews are separate caches: clearing them does not evict originals.
        // A second held read is direct evidence the LRU actually evicted the reimported original.
        await command(page, 'library.clearPreviews');
        await command(page, 'library.select', {ids: [photo.id], active: photo.id});
        // Renderer textures survive clearPreviews. A normal edit gives the selected photo a
        // fresh render key, so a retained texture cannot satisfy the pending-read assertions.
        await command(page, 'develop.set', {control: 'light.exposure', value: -1});
        await command(page, 'view.photoGrid');
        await until(() => page.evaluate(() => readRace.callbacks.length === 2));
        const pending = await command(page, 'web.stats');
        assert.equal(pending.activeThumbnailReady, false); assert.ok(pending.renderQueue > 0);
        assert.equal(pending.workers.alive, 0); assert.equal(pending.workers.remoteJobs, 0);
        const beforeNotice = await page.locator('#lightcraft_recovery_status').textContent();
        await page.evaluate(fail => readRace.callbacks[0].release(fail, 'fixture obsolete read refusal'), oldFailure);
        // Commands cross successive real animation frames after the obsolete callback settles.
        for (let frame = 0; frame < 6; frame++) {
          const s = await command(page, 'web.stats');
          assert.equal(s.originalBytesInMemory, pending.originalBytesInMemory, 'obsolete success cannot refill the LRU');
          assert.equal(s.activeThumbnailReady, false); assert.ok(s.renderQueue > 0, 'new read still owns pending render');
        }
        assert.equal(await page.locator('#lightcraft_recovery_status').textContent(), beforeNotice, 'obsolete failures cannot publish a notice');
        assert.equal(await page.evaluate(() => readRace.requests.length), 2, 'obsolete callback cannot clear newer loading');
        await page.evaluate(fail => readRace.callbacks[1].release(fail, 'fixture current read refusal'), currentFailure);
        if (currentFailure) {
          await page.waitForFunction(() => document.getElementById('lightcraft_recovery_status').textContent.includes('fixture current read refusal'));
          // An old displayed texture can remain after an error. The synchronous pixel command
          // must expose this current original failure, independently of retained display pixels.
          await assert.rejects(() => command(page, 'app.export', {format: 'png', path: '/exports/current-failure.png'}), /fixture current read refusal/);
          for (let frame = 0; frame < 6; frame++) await command(page, 'web.stats');
          assert.equal(await page.evaluate(() => readRace.requests.length), 2, 'current failure is terminal');
          await pick({name: 'read-race.png', mimeType: 'image/png', buffer: bytes});
          await until(async () => (await command(page, 'web.stats')).originalBytesInMemory > pending.originalBytesInMemory);
          await command(page, 'library.clearPreviews');
          await command(page, 'library.select', {ids: [photo.id], active: photo.id});
          await command(page, 'develop.set', {control: 'light.exposure', value: 0});
        }
        await until(async () => (await command(page, 'web.stats')).activeThumbnailReady);
        await checkExport(page, currentFailure ? 255 : 188);
        console.log(`PASS actual original-read race ${fallback ? 'all-dead' : 'workers=0'}/${oldFailure ? 'old failure' : 'old success'}/${currentFailure ? 'current failure + reimport' : 'current success'}: real LRU, owned callbacks, selected pixels`);
      } finally { await context.close(); }
    }
  } finally { await rm(directory, {recursive: true}); }
}
