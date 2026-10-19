// Parent-run integration checks against an already built, locally served browser bundle:
//   node apps/lightcraft-web/tests/browser-lifecycle.mjs http://127.0.0.1:8080/
// LIGHTCRAFT_PLAYWRIGHT_MODULE may point to the parent's installed Playwright module.
// This script builds nothing; contexts are isolated and all media/archives are synthetic.
// For original-read reimport/LRU races (about 800 MiB, unchanged production budget), set
// LIGHTCRAFT_ORIGINAL_READ_RACES=required and LIGHTCRAFT_ORIGINAL_RACE_SCRATCH=<private dir>.
// WEB-alone runs no thumbnail-plan commands. For the combined feature harness, set
// LIGHTCRAFT_THUMBNAIL_LIFECYCLE=required (or 1 to skip explicitly when unavailable).
import assert from 'node:assert/strict';
import {readBackup} from './backup-archive.mjs';
const {chromium} = await import(process.env.LIGHTCRAFT_PLAYWRIGHT_MODULE || 'playwright');
const base = new URL(process.argv[2] || 'http://127.0.0.1:8080/');
assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname), 'use a local test server');
const browser = await chromium.launch({headless: true});
const command = (page, id, params = {}) => page.evaluate(async ({id, params}) =>
  JSON.parse(await lightcraft.command(id, JSON.stringify(params))), {id, params});
async function until(check, timeout = 120000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    if (await check()) return;
    await new Promise(resolve => setTimeout(resolve, 200));
  }
  throw new Error('browser condition did not settle');
}
async function opened(context, query) {
  const page = await context.newPage();
  page.setDefaultTimeout(120000);
  page.setDefaultNavigationTimeout(120000);
  const url = new URL(base); url.search = query;
  await page.goto(url.href);
  await page.waitForFunction(() => globalThis.lightcraft?.command);
  await command(page, 'web.stats');
  await page.locator('#lightcraft_loading').waitFor({state: 'hidden'});
  return page;
}
async function download(page, action, expectedName) {
  const pending = page.waitForEvent('download');
  await action();
  const result = await pending;
  assert.equal(await result.failure(), null);
  if (expectedName) assert.equal(result.suggestedFilename(), expectedName);
  const chunks = [];
  for await (const chunk of await result.createReadStream()) chunks.push(chunk);
  return Buffer.concat(chunks);
}
async function png(page, color = 'red') {
  return Buffer.from(await page.evaluate(color => {
    const c = document.createElement('canvas'); c.width = c.height = 128;
    const ctx = c.getContext('2d'); ctx.fillStyle = 'white'; ctx.fillRect(0, 0, 128, 128);
    ctx.fillStyle = color; ctx.fillRect(0, 0, 32, 32);
    return c.toDataURL('image/png').split(',')[1];
  }, color), 'base64');
}
async function selectFixture(page, name = 'lifecycle.png') {
  let photo;
  await until(async () => {
    photo = (await command(page, 'catalog.query', {filter: {}, limit: 200})).photos.find(p => p.fileName === name);
    return !!photo;
  });
  await command(page, 'library.select', {ids: [photo.id], active: photo.id});
  await until(async () => (await command(page, 'web.stats')).originalsInMemory > 0);
  return photo;
}
async function importFixture(page, name = 'lifecycle.png', color = 'red') {
  const buffer = await png(page, color);
  const pending = page.waitForEvent('filechooser');
  await command(page, 'file.addPhotos');
  await (await pending).setFiles({name, mimeType: 'image/png', buffer});
  const photo = await selectFixture(page, name);
  await until(async () => { const s = await command(page, 'web.stats'); return s.storage === 'memory' || (!s.dirty && !s.flushing); });
  return photo;
}
async function checkExport(page, value = 255, action, expectedName) {
  const bytes = await download(page, action || (() => command(page, 'app.export', {format: 'png', path: '/exports/lifecycle.png'})), expectedName);
  const samples = await page.evaluate(async b64 => {
    const bytes = Uint8Array.from(atob(b64), c => c.charCodeAt(0));
    const bitmap = await createImageBitmap(new Blob([bytes], {type: 'image/png'}));
    const c = document.createElement('canvas'); c.width = bitmap.width; c.height = bitmap.height;
    const ctx = c.getContext('2d'); ctx.drawImage(bitmap, 0, 0);
    return [Array.from(ctx.getImageData(8, 8, 1, 1).data), Array.from(ctx.getImageData(96, 96, 1, 1).data)];
  }, bytes.toString('base64'));
  for (const [sample, expected] of [[samples[0], [value, 0, 0, 255]], [samples[1], [value, value, value, 255]]]) {
    sample.forEach((v, i) => assert.ok(Math.abs(v - expected[i]) <= 2, `export sample ${sample}, expected ${expected}`));
  }
}
// Only this isolated test context's healthy imported original is removed. Reload drops all
// decoded/original caches, making the inactive photo genuinely unavailable without an archive.
async function removeFixtureOriginal(page, id) {
  const photo = await command(page, 'photo.inspect', {id});
  assert.equal(photo.source.type, 'file');
  const hash = photo.source.path.match(/^web\/([^/]+)\//)?.[1];
  assert.ok(hash, 'fixture must use browser original storage');
  await page.evaluate(key => new Promise((resolve, reject) => {
    const request = indexedDB.open('lightcraft');
    request.onerror = () => reject(request.error);
    request.onsuccess = () => {
      const db = request.result;
      const tx = db.transaction('files', 'readwrite');
      tx.oncomplete = () => { db.close(); resolve(); };
      tx.onerror = tx.onabort = () => { db.close(); reject(tx.error || new Error('fixture removal aborted')); };
      const files = tx.objectStore('files');
      const original = files.get(key);
      original.onsuccess = () => {
        if (original.result == null) { tx.abort(); return; }
        files.delete(key);
      };
    };
  }), `originals/${hash}`);
}

async function workerFailure(variant) {
  const context = await browser.newContext({acceptDownloads: true});
  try {
    await context.addInitScript(kind => {
      const Original = Worker; let attempted = 0;
      globalThis.workerAudit = [];
      globalThis.Worker = class extends Original {
        constructor(url, options) {
          const attempt = attempted++;
          if (kind === 'constructor' && attempt === 0) throw new DOMException('synthetic failure', 'SecurityError');
          super(url, options); this.attempt = attempt;
          this.addEventListener('message', e => workerAudit.push({attempt, ready: e.data?.ready, id: e.data?.id}));
        }
        postMessage(data, ...rest) {
          if (kind === 'init' && this.attempt === 0 && data?.module instanceof WebAssembly.Module) {
            throw new DOMException('synthetic init failure', 'DataCloneError');
          }
          return super.postMessage(data, ...rest);
        }
      };
    }, variant);
    const page = await opened(context, 'store=idb&workers=4');
    await until(async () => {
      const s = await command(page, 'web.stats');
      return s.workers.ready === 3 && s.workers.remoteJobs > 0 && s.renderQueue === 0 && s.rendersInFlight === 0;
    });
    const audit = await page.evaluate(() => workerAudit);
    assert.deepEqual(audit.filter(e => e.ready).map(e => e.attempt).sort(), [1, 2, 3]);
    assert.equal((await command(page, 'web.stats')).workers.alive, 3);
    assert.ok(audit.some(e => e.id !== undefined));
    console.log(`PASS worker ${variant}: three survivors ready, jobs complete`);
  } finally { await context.close(); }
}
// Worker-only overrides leave the page's healthy backend and imported PNG intact.
async function workerStorageFailure(kind, failures = 'all') {
  const context = await browser.newContext({acceptDownloads: true});
  try {
    await context.addInitScript(() => {
      globalThis.workerAudit = [];
      const Original = Worker;
      globalThis.Worker = class extends Original {
        constructor(url, options) {
          super(url, options);
          this.addEventListener('message', e => workerAudit.push({ready: e.data?.ready, store: e.data?.store, initError: e.data?.initError}));
        }
      };
    });
    await context.route('**/worker.js', async route => {
      const response = await route.fetch();
      const prefix = kind === 'idb' ? `
        const open = IDBFactory.prototype.open;
        IDBFactory.prototype.open = function(name, ...args) {
          if (name === 'lightcraft') throw new DOMException('fixture worker storage refusal', 'UnknownError');
          return open.call(this, name, ...args);
        };` : kind === 'opfs' ? `
        if (navigator.storage) {
          Object.defineProperty(navigator.storage, 'getDirectory', {configurable: true,
            value() { return Promise.reject(new DOMException('fixture worker OPFS root refusal', 'NotReadableError')); }});
        }` : `
        if (typeof FileSystemFileHandle !== 'undefined') {
          Object.defineProperty(FileSystemFileHandle.prototype, 'createWritable', {configurable: true, value: undefined});
        }`;
      const selected = failures === 'one' ? 'self.name === "lightcraft-render-0"' : 'true';
      await route.fulfill({response, body: `if (${selected}) {${prefix}}\n` + await response.text()});
    });
    const page = await opened(context, kind === 'idb' ? 'store=idb&workers=2' : 'workers=2');
    if (kind !== 'idb' && (await command(page, 'web.stats')).storage !== 'opfs') {
      console.log(`SKIP worker ${kind} storage refusal: page does not support OPFS`);
      return;
    }
    const survivors = failures === 'one' ? 1 : 0;
    await until(async () => {const s = await command(page, 'web.stats'); return s.workers.alive === survivors && s.workers.ready === survivors;});
    assert.match(await page.locator('#lightcraft_recovery_status').textContent(), /background worker|storage/);
    const counter = survivors ? 'remoteJobs' : 'inlineJobs';
    const before = (await command(page, 'web.stats')).workers[counter];
    await importFixture(page, 'worker-storage.png');
    await command(page, 'view.detail');
    await until(async () => {
      const s = await command(page, 'web.stats');
      return s.workers[counter] > before && s.renderQueue === 0 && s.rendersInFlight === 0;
    });
    await checkExport(page);
    const audit = await page.evaluate(() => workerAudit);
    assert.equal(audit.filter(e => e.ready).length, survivors);
    assert.equal(audit.filter(e => e.initError).length, 2 - survivors);
    for (const ready of audit.filter(e => e.ready)) assert.equal(ready.store, (await command(page, 'web.stats')).storage);
    console.log(`PASS worker ${kind}/${failures} storage refusal: no false ready, retired workers, healthy original completes via ${counter}`);
  } finally { await context.close(); }
}

// Keep the valid PNG in IDB. Hold only its native request callback so the normal renderer
// must defer while bytes arrive; read refusal likewise leaves the stored original untouched.
async function inlineReloadRendering(fallback, terminal = false, extension = null) {
  const context = await browser.newContext({acceptDownloads: true});
  try {
    await context.addInitScript(() => {
      globalThis.originalReads = [];
      globalThis.originalReadCallbacks = [];
      const held = new WeakSet();
      const get = IDBObjectStore.prototype.get;
      IDBObjectStore.prototype.get = function(key) {
        const mode = sessionStorage.getItem('inline-original-mode');
        if (this.name === 'files' && typeof key === 'string' && key.startsWith('originals/') && mode) {
          originalReads.push(key);
          if (mode === 'refuse') throw new DOMException('fixture original read refusal', 'NotReadableError');
          const request = get.call(this, key); held.add(request); return request;
        }
        return get.call(this, key);
      };
      const success = Object.getOwnPropertyDescriptor(IDBRequest.prototype, 'onsuccess');
      if (!success?.set) throw new Error('fixture requires native IDB request onsuccess');
      Object.defineProperty(IDBRequest.prototype, 'onsuccess', {configurable: true, get: success.get, set(callback) {
        const handler = held.has(this) && callback ? function(event) {
          const request = this;
          originalReadCallbacks.push(() => callback.call(request, event));
        } : callback;
        success.set.call(this, handler);
      }});
    });
    if (fallback) await context.route('**/worker.js', async route => {
      const response = await route.fetch();
      const prefix = `const open = IDBFactory.prototype.open;
        IDBFactory.prototype.open = function(name, ...args) {
          if (name === 'lightcraft') throw new DOMException('fixture worker storage refusal', 'UnknownError');
          return open.call(this, name, ...args);
        };`;
      await route.fulfill({response, body: prefix + '\n' + await response.text()});
    });
    const page = await opened(context, 'store=idb&workers=0');
    const photo = await importFixture(page, 'inline-reload.png');
    await command(page, 'view.photoGrid');
    await command(page, 'library.cancelPreviews');
    await until(async () => {const s = await command(page, 'web.stats'); return s.renderQueue === 0 && s.rendersInFlight === 0;});
    await command(page, 'library.clearPreviews');
    await until(async () => {const s = await command(page, 'web.stats'); return !s.dirty && !s.flushing;});
    await page.evaluate(mode => sessionStorage.setItem('inline-original-mode', mode), terminal ? 'refuse' : 'hold');
    const url = new URL(base); url.search = fallback ? 'store=idb&workers=2' : 'store=idb&workers=0';
    await page.goto(url.href);
    await page.waitForFunction(() => globalThis.lightcraft?.command);
    await page.locator('#lightcraft_loading').waitFor({state: 'hidden'});
    if (fallback) await until(async () => {const s = await command(page, 'web.stats'); return s.workers.alive === 0 && s.workers.ready === 0;});
    await command(page, 'view.photoGrid');
    await command(page, 'library.select', {ids: [photo.id], active: photo.id});
    const plan = extension ? await extension({page, command, until}) : null;
    if (!terminal) {
      await until(() => page.evaluate(() => originalReadCallbacks.length > 0));
      const pending = await command(page, 'web.stats');
      assert.equal(pending.originalsInMemory, 0, 'the healthy original is still held');
      assert.equal(pending.activeThumbnailReady, false, 'held bytes cannot produce the selected thumbnail');
      assert.ok(pending.renderQueue > 0, 'existing Grid renders defer while bytes arrive');
      if (plan) await plan.held();
      await page.evaluate(() => {for (const finish of originalReadCallbacks.splice(0)) finish();});
    }
    if (plan) await plan.complete(terminal);
    await until(async () => {
      const s = await command(page, 'web.stats');
      return s.workers.inlineJobs > 0 && s.renderQueue === 0 && s.rendersInFlight === 0;
    });
    const stats = await command(page, 'web.stats');
    assert.equal(stats.workers.remoteJobs, 0);
    assert.equal(stats.workers.alive, 0);
    if (terminal) {
      assert.equal(stats.activeThumbnailReady, false, 'a refused original cannot produce the selected thumbnail');
      const notice = await page.locator('#lightcraft_recovery_status').textContent();
      assert.match(notice, /fixture original read refusal/);
      assert.doesNotMatch(notice, /still loading/);
      for (let frame = 0; frame < 4; frame++) await command(page, 'web.stats');
      assert.equal((await page.evaluate(() => originalReads)).length, 1, 'terminal reads must not requeue every frame');
      await page.evaluate(() => sessionStorage.removeItem('inline-original-mode'));
      await page.reload(); await page.waitForFunction(() => globalThis.lightcraft?.command);
      await selectFixture(page, 'inline-reload.png'); await checkExport(page);
    } else {
      assert.equal(stats.activeThumbnailReady, true, 'the selected healthy fixture must have a rendered thumbnail');
      assert.equal((await page.evaluate(() => originalReads)).length, 1);
      await command(page, 'library.select', {ids: [photo.id], active: photo.id});
      await checkExport(page);
    }
    console.log(`PASS inline reload ${fallback ? 'all-dead' : 'workers=0'}/${terminal ? 'terminal refusal' : 'healthy held read'}: bounded readiness and existing Grid results${plan ? " plus thumbnail progress" : ""}`);
  } finally { await context.close(); }
}

// Startup must settle visibly when a present Web Locks API fails without invoking its
// grant callback. These overrides affect only this isolated browser context.
async function lockFailure(variant) {
  const context = await browser.newContext();
  try {
    await context.addInitScript(kind => {
      globalThis.lockRequests = 0;
      Object.defineProperty(navigator, 'locks', {configurable: true, value: {
        request() {
          lockRequests++;
          const error = new DOMException('synthetic lock denial', 'SecurityError');
          if (kind === 'throw') throw error;
          return Promise.reject(error);
        },
      }});
    }, variant);
    const page = await context.newPage();
    page.setDefaultTimeout(120000);
    page.setDefaultNavigationTimeout(120000);
    const url = new URL(base); url.search = 'store=idb&workers=0';
    await page.goto(url.href);
    const blocking = page.locator('#lightcraft_blocking');
    await blocking.waitFor({state: 'visible'});
    assert.match(await blocking.textContent(), /could not lock the library/);
    assert.match(await blocking.textContent(), /synthetic lock denial/);
    assert.match(await blocking.textContent(), /store=memory/);
    assert.equal(await page.locator('#lightcraft_loading').count(), 0);
    assert.equal(await page.evaluate(() => lockRequests), 1);
    await page.close();
    const temporary = await opened(context, 'store=memory&workers=0');
    assert.equal(await temporary.evaluate(() => lockRequests), 0);
    assert.equal((await command(temporary, 'web.stats')).storage, 'memory');
    console.log(`PASS Web Locks ${variant}: visible startup error, temporary session bypasses locking`);
  } finally { await context.close(); }
}
// Temporary sessions keep imported originals on the main instance, so the default worker
// setting must use inline rendering. The healthy fixture must still export after another import.
async function temporaryOriginals() {
  const context = await browser.newContext({acceptDownloads: true});
  try {
    const page = await opened(context, 'store=memory');
    const first = await importFixture(page);
    await command(page, 'view.detail');
    await until(async () => {
      const s = await command(page, 'web.stats');
      return s.renderQueue === 0 && s.rendersInFlight === 0;
    });
    assert.equal((await command(page, 'web.stats')).workers.alive, 0);
    await checkExport(page);
    await importFixture(page, 'second.png', 'blue');
    await command(page, 'library.select', {ids: [first.id], active: first.id});
    await checkExport(page);
    console.log('PASS default temporary session: imported PNG renders inline and remains exportable');
  } finally { await context.close(); }
}
async function rendererFailure() {
  const context = await browser.newContext();
  try {
    await context.addInitScript(() => {
      const get = HTMLCanvasElement.prototype.getContext;
      HTMLCanvasElement.prototype.getContext = function(type, ...args) {
        if (this.id === 'lightcraft_canvas' && ['webgl2', 'webgl', 'experimental-webgl'].includes(type)) return null;
        return get.call(this, type, ...args);
      };
    });
    const page = await context.newPage();
    page.setDefaultTimeout(120000);
    const url = new URL(base); url.search = 'store=memory';
    await page.goto(url.href);
    const blocking = page.locator('#lightcraft_blocking');
    await blocking.waitFor({state: 'visible'});
    assert.match(await blocking.textContent(), /could not start the editor/);
    assert.match(await blocking.textContent(), /Reload to try again/);
    assert.equal(await page.locator('#lightcraft_loading').count(), 0);
    console.log('PASS renderer refusal: visible blocking error with reload guidance');
  } finally { await context.close(); }
}
// These contexts contain only healthy fixtures. A transient read rejection must stop startup
// before catalog writes; removing the rejection must reopen the same stored rating on retry.
async function startupReadFailure(key) {
  const context = await browser.newContext({acceptDownloads: true});
  try {
    await context.addInitScript(() => {
      globalThis.libraryWrites = [];
      const get = IDBObjectStore.prototype.get;
      const put = IDBObjectStore.prototype.put;
      IDBObjectStore.prototype.get = function(key) {
        if (this.name === 'files' && sessionStorage.getItem('read-failure') === key) {
          throw new DOMException('synthetic storage read refusal', 'NotReadableError');
        }
        return get.call(this, key);
      };
      IDBObjectStore.prototype.put = function(value, key) {
        if (this.name === 'files' && (key === 'active-library' || /catalog\.(snap|log)$/.test(key))) libraryWrites.push(key);
        return put.call(this, value, key);
      };
    });
    const page = await opened(context, 'store=idb&workers=0');
    const photo = await importFixture(page);
    await command(page, 'photo.rate', {rating: 4});
    await until(async () => {const s = await command(page, 'web.stats'); return !s.dirty && !s.flushing;});
    await page.evaluate(key => sessionStorage.setItem('read-failure', key), key);
    await page.reload();
    const blocking = page.locator('#lightcraft_blocking');
    await blocking.waitFor({state: 'visible'});
    assert.match(await blocking.textContent(), /could not read the library/);
    assert.match(await blocking.textContent(), /synthetic storage read refusal/);
    assert.deepEqual(await page.evaluate(() => libraryWrites), []);
    assert.equal(await page.locator('#lightcraft_loading').count(), 0);
    await page.evaluate(() => sessionStorage.removeItem('read-failure'));
    await page.reload();
    await page.waitForFunction(() => globalThis.lightcraft?.command);
    await command(page, 'web.stats');
    await page.locator('#lightcraft_loading').waitFor({state: 'hidden'});
    assert.equal((await command(page, 'photo.inspect', {id: photo.id})).rating, 4);
    console.log(`PASS startup read refusal ${key}: no catalog writes, same library restored on retry`);
  } finally { await context.close(); }
}

// Root-open failure must not open or write an alternative backend, including when IDB works.
async function opfsRootFailure(variant) {
  const context = await browser.newContext({acceptDownloads: true});
  try {
    await context.addInitScript(kind => {
      globalThis.idbOpens = [];
      globalThis.libraryWrites = [];
      const open = IDBFactory.prototype.open;
      IDBFactory.prototype.open = function(name, ...args) {
        if (name === 'lightcraft') idbOpens.push(name);
        return open.call(this, name, ...args);
      };
      const put = IDBObjectStore.prototype.put;
      IDBObjectStore.prototype.put = function(value, key) {
        if (this.name === 'files') libraryWrites.push(key);
        return put.call(this, value, key);
      };
      if (!navigator.storage?.getDirectory || typeof FileSystemFileHandle === 'undefined') return;
      const get = navigator.storage.getDirectory.bind(navigator.storage);
      Object.defineProperty(navigator.storage, 'getDirectory', {configurable: true, value() {
        if (sessionStorage.getItem('opfs-root-failure')) {
          const error = new DOMException('fixture OPFS root refusal', 'NotReadableError');
          if (kind === 'throw') throw error;
          return Promise.reject(error);
        }
        return get();
      }});
      const write = FileSystemFileHandle.prototype.createWritable;
      if (typeof write === 'function') FileSystemFileHandle.prototype.createWritable = function(options) {
        libraryWrites.push(this.name);
        return write.call(this, options);
      };
    }, variant);
    const page = await opened(context, 'workers=0');
    if ((await command(page, 'web.stats')).storage !== 'opfs') {
      console.log(`SKIP OPFS root ${variant}: this browser selected IndexedDB`);
      return;
    }
    const photo = await importFixture(page);
    await command(page, 'photo.rate', {rating: 4});
    await until(async () => {const s = await command(page, 'web.stats'); return !s.dirty && !s.flushing;});
    await page.evaluate(() => sessionStorage.setItem('opfs-root-failure', 'yes'));
    await page.reload();
    const blocking = page.locator('#lightcraft_blocking');
    await blocking.waitFor({state: 'visible'});
    assert.match(await blocking.textContent(), /fixture OPFS root refusal/);
    assert.match(await blocking.textContent(), /Reload.*retry|Reload.*try again/i);
    assert.deepEqual(await page.evaluate(() => idbOpens), []);
    assert.deepEqual(await page.evaluate(() => libraryWrites), []);
    assert.equal(await page.locator('#lightcraft_loading').count(), 0);
    await page.evaluate(() => sessionStorage.removeItem('opfs-root-failure'));
    await page.reload();
    await page.waitForFunction(() => globalThis.lightcraft?.command);
    await page.locator('#lightcraft_loading').waitFor({state: 'hidden'});
    assert.equal((await command(page, 'web.stats')).storage, 'opfs');
    assert.equal((await command(page, 'photo.inspect', {id: photo.id})).rating, 4);
    console.log(`PASS OPFS root ${variant}: visible error, no backend switch/write, healthy library returns on retry`);
  } finally { await context.close(); }
}

async function opfsReadFailure(lookup) {
  const context = await browser.newContext({acceptDownloads: true});
  try {
    await context.addInitScript(() => {
      globalThis.libraryWrites = [];
      if (typeof FileSystemDirectoryHandle === 'undefined' || typeof FileSystemFileHandle === 'undefined') return;
      const directory = FileSystemDirectoryHandle.prototype.getDirectoryHandle;
      const file = FileSystemDirectoryHandle.prototype.getFileHandle;
      const write = FileSystemFileHandle.prototype.createWritable;
      if (typeof write !== 'function') return;
      FileSystemDirectoryHandle.prototype.getDirectoryHandle = function(name, options) {
        if (!options?.create && name === 'library' && sessionStorage.getItem('opfs-read-failure') === 'directory') {
          return Promise.reject(new DOMException('synthetic OPFS read refusal', 'NotReadableError'));
        }
        return directory.call(this, name, options);
      };
      FileSystemDirectoryHandle.prototype.getFileHandle = function(name, options) {
        if (!options?.create && name === 'catalog.snap' && sessionStorage.getItem('opfs-read-failure') === 'file') {
          return Promise.reject(new DOMException('synthetic OPFS read refusal', 'NotReadableError'));
        }
        return file.call(this, name, options);
      };
      FileSystemFileHandle.prototype.createWritable = function(options) {
        if (/^catalog\.(snap|log)$/.test(this.name)) libraryWrites.push(this.name);
        return write.call(this, options);
      };
    });
    const page = await opened(context, 'workers=0');
    if ((await command(page, 'web.stats')).storage !== 'opfs') {
      console.log(`SKIP OPFS ${lookup} read refusal: this browser selected IndexedDB`);
      return;
    }
    const photo = await importFixture(page);
    await command(page, 'photo.rate', {rating: 4});
    await until(async () => {const s = await command(page, 'web.stats'); return !s.dirty && !s.flushing;});
    await page.evaluate(lookup => sessionStorage.setItem('opfs-read-failure', lookup), lookup);
    await page.reload();
    const blocking = page.locator('#lightcraft_blocking');
    await blocking.waitFor({state: 'visible'});
    assert.match(await blocking.textContent(), /could not read the library/);
    assert.match(await blocking.textContent(), /synthetic OPFS read refusal/);
    assert.deepEqual(await page.evaluate(() => libraryWrites), []);
    await page.evaluate(() => sessionStorage.removeItem('opfs-read-failure'));
    await page.reload();
    await page.waitForFunction(() => globalThis.lightcraft?.command);
    await command(page, 'web.stats');
    await page.locator('#lightcraft_loading').waitFor({state: 'hidden'});
    assert.equal((await command(page, 'photo.inspect', {id: photo.id})).rating, 4);
    console.log(`PASS OPFS ${lookup} read refusal: error differs from absence, no catalog writes, retry opens healthy library`);
  } finally { await context.close(); }
}

async function assertBackingWarning(page) {
  await until(async () => {
    const s = await command(page, 'web.stats');
    return s.saveError && s.unsaved?.error && s.unsavedIndicator && /not written to disk/.test(s.toast);
  });
  const initial = await command(page, 'web.stats');
  assert.match(initial.toast, /fixture backing write refusal|synthetic full library store/);
  for (let frame = 0; frame < 4; frame++) {
    const s = await command(page, 'web.stats');
    assert.ok(s.unsaved && s.unsavedIndicator && s.saveError, 'read-only queries cannot clear a backing warning');
  }
  assert.equal(await page.evaluate(() => {
    const event = new Event('beforeunload', {cancelable: true});
    window.dispatchEvent(event);
    return event.defaultPrevented;
  }), true, 'failed backing writes must request the browser reload confirmation');
}

async function backingSaveWarning(startup = false) {
  const context = await browser.newContext({acceptDownloads: true});
  try {
    await context.addInitScript(startup => {
      globalThis.refuseCatalogWrites = startup && sessionStorage.getItem('backing-write-recovered') !== 'yes';
      const put = IDBObjectStore.prototype.put;
      IDBObjectStore.prototype.put = function(value, key) {
        if (refuseCatalogWrites && this.name === 'files' && /catalog\.(snap|log)$/.test(key)) {
          throw new DOMException('fixture backing write refusal', 'QuotaExceededError');
        }
        return put.call(this, value, key);
      };
    }, startup);
    const page = await opened(context, 'store=idb&workers=0');
    let photo;
    if (!startup) {
      photo = await importFixture(page, 'backing-retry.png');
      await page.evaluate(() => { refuseCatalogWrites = true; });
      await command(page, 'photo.rate', {rating: 4}); // accepted by the mirror before the asynchronous failure
    }
    await assertBackingWarning(page); // no subsequent catalog edit
    await page.evaluate(() => {
      refuseCatalogWrites = false;
      sessionStorage.setItem('backing-write-recovered', 'yes');
    });
    await until(async () => {
      const s = await command(page, 'web.stats');
      return !s.saveError && !s.unsaved && !s.dirty && !s.flushing && !s.unsavedIndicator && s.toast === 'Library saved';
    });
    assert.equal(await page.evaluate(() => {
      const event = new Event('beforeunload', {cancelable: true});
      window.dispatchEvent(event);
      return event.defaultPrevented;
    }), false, 'a successful flush clears the reload confirmation');
    await page.reload();
    await page.waitForFunction(() => globalThis.lightcraft?.command);
    await until(async () => {
      const s = await command(page, 'web.stats');
      return !s.saveError && !s.unsaved && !s.dirty && !s.flushing && !s.unsavedIndicator;
    });
    if (photo) {
      await selectFixture(page, 'backing-retry.png');
      assert.equal((await command(page, 'photo.inspect', {id: photo.id})).rating, 4);
      await checkExport(page);
    } else {
      assert.ok((await command(page, 'catalog.query', {filter: {}, limit: 200})).photos.length > 0);
    }
    console.log(`PASS ${startup ? 'startup snapshot' : 'accepted append'} backing failure: existing warning, toast, reload protection, retry and persisted reload`);
  } finally { await context.close(); }
}

async function unsavedBackup() {
  const source = await browser.newContext({acceptDownloads: true});
  try {
    await source.addInitScript(() => {
      globalThis.refuseCatalogWrites = false;
      const put = IDBObjectStore.prototype.put;
      IDBObjectStore.prototype.put = function(value, key) {
        if (refuseCatalogWrites && this.name === 'files' && /catalog\.(snap|log)$/.test(key)) {
          throw new DOMException('synthetic full library store', 'QuotaExceededError');
        }
        return put.call(this, value, key);
      };
    });
    const page = await opened(source, 'store=idb&workers=0');
    await importFixture(page);
    await page.evaluate(() => { refuseCatalogWrites = true; });
    await command(page, 'photo.rate', {rating: 4});
    await assertBackingWarning(page);
    await assert.rejects(() => command(page, 'photo.rate', {rating: 5}), /not written/);
    await assert.rejects(() => command(page, 'develop.set', {control: 'light.exposure', value: -1}), /not written/);
    const before = await command(page, 'library.info');
    assert.ok(before.unsavedOps > 0);
    const healthy = await download(page, () => command(page, 'file.backupLibrary'));
    const complete = readBackup(healthy);
    assert.ok(complete.has('library/catalog.snap') && complete.has('library/catalog.log'));
    assert.equal(complete.has('MISSING-ORIGINALS.txt'), false, 'failed saves do not make complete originals partial');
    assert.equal((await command(page, 'library.info')).unsavedOps, before.unsavedOps);
    const target = await browser.newContext({acceptDownloads: true});
    try {
      const restored = await opened(target, 'store=idb&workers=0');
      await restore(restored, healthy, 188);
      const photo = (await command(restored, 'catalog.query', {filter: {}, limit: 200})).photos.find(p => p.fileName === 'lifecycle.png');
      assert.equal((await command(restored, 'photo.inspect', {id: photo.id})).rating, 5);
      console.log('PASS failed-store backup/restore: live rating and exposure retained without clearing unsaved edits');
    } finally { await target.close(); }
    await page.evaluate(() => { refuseCatalogWrites = false; });
    await until(async () => {
      const s = await command(page, 'web.stats');
      return !s.saveError && !s.unsaved && !s.dirty && !s.flushing && !s.unsavedIndicator;
    });
    await page.reload(); await page.waitForFunction(() => globalThis.lightcraft?.command);
    const saved = await selectFixture(page);
    assert.equal((await command(page, 'photo.inspect', {id: saved.id})).rating, 5);
    await checkExport(page, 188);
    console.log('PASS failed-store retry: queued live edits reach backing storage before the warning clears');
  } finally { await source.close(); }
}

async function restore(page, bytes, value = 255) {
  page.once('dialog', dialog => dialog.accept());
  const picker = page.waitForEvent('filechooser');
  await command(page, 'file.restoreLibrary');
  const reload = page.waitForEvent('load');
  await (await picker).setFiles({name: 'healthy-backup.zip', mimeType: 'application/zip', buffer: bytes});
  await reload; await page.waitForFunction(() => globalThis.lightcraft?.command);
  await page.locator('#lightcraft_loading').waitFor({state: 'hidden'});
  await selectFixture(page); await checkExport(page, value);
}

try {
  if (process.env.LIGHTCRAFT_ORIGINAL_READ_RACES === 'required') {
    const {runOriginalReadRaces} = await import('./original-read-races.mjs');
    await runOriginalReadRaces({browser, base, command, until, opened, png, checkExport});
  }
  await workerFailure('constructor');
  await workerFailure('init');
  for (const kind of ['idb', 'opfs', 'unsupported-opfs']) await workerStorageFailure(kind);
  await workerStorageFailure('idb', 'one');
  await inlineReloadRendering(false);
  await inlineReloadRendering(true);
  await inlineReloadRendering(false, true);
  const thumbnailMode = process.env.LIGHTCRAFT_THUMBNAIL_LIFECYCLE || 'off';
  assert.ok(['off', '1', 'required'].includes(thumbnailMode), 'invalid thumbnail lifecycle mode');
  if (thumbnailMode !== 'off') {
    const probe = await browser.newContext();
    let available;
    try {
      const page = await opened(probe, 'store=memory&workers=0');
      available = (await command(page, 'engine.commands')).some(c => c.id === 'library.buildThumbnails');
    } finally { await probe.close(); }
    if (thumbnailMode === 'required') assert.ok(available, 'combined harness requires the thumbnail feature');
    if (available) {
      const {startThumbnailPlan} = await import('./thumbnail-lifecycle.mjs');
      await inlineReloadRendering(false, false, startThumbnailPlan);
      await inlineReloadRendering(true, false, startThumbnailPlan);
      await inlineReloadRendering(false, true, startThumbnailPlan);
    } else console.log('SKIP thumbnail extension: command registry has no thumbnail feature');
  }
  await lockFailure('throw');
  await lockFailure('reject');
  await temporaryOriginals();
  await rendererFailure();
  await backingSaveWarning(true);
  await backingSaveWarning();
  await unsavedBackup();
  for (const key of ['active-library', 'library/catalog.snap', 'library/catalog.log']) await startupReadFailure(key);
  for (const variant of ['throw', 'reject']) await opfsRootFailure(variant);
  for (const lookup of ['directory', 'file']) await opfsReadFailure(lookup);
  const source = await browser.newContext({acceptDownloads: true});
  try {
    await source.addInitScript(() => {
      const get = HTMLCanvasElement.prototype.getContext;
      HTMLCanvasElement.prototype.getContext = function(type, ...args) {
        const context = get.call(this, type, ...args);
        if (this.id === 'lightcraft_canvas' && type === 'webgl2' && context) globalThis.editorGl = context;
        return context;
      };
    });
    const page = await opened(source, 'store=idb&workers=0');
    const active = await importFixture(page); await checkExport(page);
    const healthy = await download(page, () => command(page, 'file.backupLibrary'));
    const completeEntries = readBackup(healthy);
    const activeInspect = await command(page, 'photo.inspect', {id: active.id});
    const activeHash = activeInspect.source.path.match(/^web\/([^/]+)\//)?.[1];
    assert.ok(activeHash);
    assert.ok(completeEntries.has(`originals/${activeHash}/lifecycle.png`));
    assert.equal(completeEntries.has('MISSING-ORIGINALS.txt'), false);
    const restoredContext = await browser.newContext({acceptDownloads: true});
    try {
      const restored = await opened(restoredContext, 'store=idb&workers=0');
      await restore(restored, healthy);
      console.log('PASS healthy backup/restore: original pixels retained');
    } finally { await restoredContext.close(); }
    await command(page, 'develop.set', {control: 'light.exposure', value: -1});
    await checkExport(page, 188);
    const inactive = await importFixture(page, 'inactive.png', 'blue');
    assert.notEqual(inactive.id, active.id, 'fixtures must be distinct photos');
    await removeFixtureOriginal(page, inactive.id);
    await page.reload();
    await page.waitForFunction(() => globalThis.lightcraft?.command);
    await command(page, 'web.stats');
    await page.locator('#lightcraft_loading').waitFor({state: 'hidden'});
    await command(page, 'library.select', {ids: [inactive.id, active.id], active: active.id});
    await until(async () => (await command(page, 'web.stats')).originalsInMemory > 0);
    await checkExport(page, 188, () => command(page, 'app.export', {format: 'png', path: '/exports/active.png', ids: [active.id]}));
    await assert.rejects(() => command(page, 'app.export', {format: 'png', path: '/exports/inactive.png', ids: [inactive.id]}), /original.*(loading|missing|storage)/i);
    const selection = (await command(page, 'library.state')).selection;
    assert.deepEqual(selection.ids, [inactive.id, active.id]);
    assert.equal(selection.active, active.id);
    const recoveryDownloads = [];
    page.on('download', result => recoveryDownloads.push(result.suggestedFilename()));
    assert.equal(await page.evaluate(() => { globalThis.editorLoseContext = editorGl.getExtension('WEBGL_lose_context'); if (!editorLoseContext) return false; editorLoseContext.loseContext(); return true; }), true);
    await page.waitForFunction(() => editorGl.isContextLost());
    await page.locator('#lightcraft_recovery').waitFor({state: 'visible'});
    assert.equal(await page.evaluate(() => document.activeElement.id), 'lightcraft_recovery_title');
    assert.equal(await page.evaluate(() => document.getElementById('lightcraft_canvas').inert), true);
    await page.keyboard.press('Tab');
    assert.equal(await page.evaluate(() => document.activeElement.id), 'lightcraft_recovery_export');
    await page.keyboard.press('Shift+Tab');
    assert.equal(await page.evaluate(() => document.activeElement.id), 'lightcraft_recovery_reload');
    await page.keyboard.press('Delete');
    assert.deepEqual((await command(page, 'library.state')).selection, selection, 'recovery keys must not change the covered editor');
    if (process.env.LIGHTCRAFT_TEST_SCREENSHOT) await page.screenshot({path: process.env.LIGHTCRAFT_TEST_SCREENSHOT});
    assert.equal(await page.locator('#lightcraft_recovery_export').textContent(), 'Export Active Photo as PNG');
    await checkExport(page, 188, async () => {
      await page.locator('#lightcraft_recovery_export').focus();
      await page.keyboard.press('Enter');
    }, 'LightCraft.png');
    await page.waitForFunction(() => document.getElementById('lightcraft_recovery_status').textContent.includes('Export finished.'));
    assert.deepEqual(recoveryDownloads, ['LightCraft.png']);
    assert.deepEqual((await command(page, 'library.state')).selection, selection);
    console.log('PASS two-photo recovery: only active photo exported despite unavailable inactive original');
    const beforePartial = await command(page, 'web.stats');
    const protectedBackup = await download(page, () => page.locator('#lightcraft_recovery_backup').click());
    const partialEntries = readBackup(protectedBackup); // validates every local entry and CRC
    const inactiveInspect = await command(page, 'photo.inspect', {id: inactive.id});
    const missingHash = inactiveInspect.source.path.match(/^web\/([^/]+)\//)?.[1];
    assert.ok(missingHash);
    assert.ok(partialEntries.has('library/catalog.snap') && partialEntries.has('library/catalog.log'));
    assert.ok(partialEntries.has(`originals/${activeHash}/lifecycle.png`));
    assert.equal([...partialEntries.keys()].some(name => name.startsWith(`originals/${missingHash}/`)), false);
    const missingText = partialEntries.get('MISSING-ORIGINALS.txt')?.toString('utf8');
    assert.match(missingText, /PARTIAL/);
    assert.ok(missingText.includes('inactive.png') && missingText.includes(missingHash));
    await page.waitForFunction(() => document.getElementById('lightcraft_recovery_status').textContent.startsWith('PARTIAL'));
    const partialNotice = await page.locator('#lightcraft_recovery_status').textContent();
    assert.ok(partialNotice.includes('inactive.png') && partialNotice.includes(missingHash));
    assert.doesNotMatch(partialNotice, /Library backed up/);
    assert.ok(recoveryDownloads.some(name => name.endsWith('-PARTIAL.zip')));
    const afterPartial = await command(page, 'web.stats');
    assert.deepEqual(afterPartial.unsaved, beforePartial.unsaved);
    assert.equal(afterPartial.saveError, beforePartial.saveError);
    assert.deepEqual((await command(page, 'library.state')).selection, selection);
    console.log('PASS partial two-photo recovery backup: catalog/edits and available PNG retained, every CRC checked, missing name/hash qualified');
    await page.evaluate(() => editorLoseContext.restoreContext());
    await page.waitForFunction(() => !editorGl.isContextLost() && document.getElementById('lightcraft_recovery_message').textContent.includes('Reload to show the editor again'));
    assert.equal(await page.locator('#lightcraft_recovery').isVisible(), true);
    await checkExport(page, 188, () => page.locator('#lightcraft_recovery_export').click());
    await page.waitForFunction(() => document.getElementById('lightcraft_recovery_status').textContent.includes('Export finished.'));
    const downloaded = recoveryDownloads.length;
    await command(page, 'library.select', {ids: []});
    await page.locator('#lightcraft_recovery_export').click();
    await page.waitForFunction(() => document.getElementById('lightcraft_recovery_status').textContent.includes('No active photo to export.'));
    assert.equal(await page.locator('#lightcraft_recovery_export').isEnabled(), true);
    assert.equal(recoveryDownloads.length, downloaded, 'no active photo must not fall back to another selected/exported photo');
    console.log('PASS recovery with no active photo: visible error, no download, button retryable');
    console.log('PASS WebGL loss/restoration: visible DOM recovery, correct exports and completed backup');
  } finally { await source.close(); }
} finally { await browser.close(); }
