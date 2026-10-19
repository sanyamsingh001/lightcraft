// Parent-run integration checks against an already built, locally served browser bundle:
//   node apps/lightcraft-web/tests/browser-lifecycle.mjs http://127.0.0.1:8080/
// LIGHTCRAFT_PLAYWRIGHT_MODULE may point to the parent's installed Playwright module.
// This script builds nothing; contexts are isolated and all media/archives are synthetic.
import assert from 'node:assert/strict';
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
  await until(async () => { const s = await command(page, 'web.stats'); return !s.dirty && !s.flushing; });
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
    const page = await opened(context, 'store=memory&workers=4');
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
async function restore(page, bytes) {
  page.once('dialog', dialog => dialog.accept());
  const picker = page.waitForEvent('filechooser');
  await command(page, 'file.restoreLibrary');
  const reload = page.waitForEvent('load');
  await (await picker).setFiles({name: 'healthy-backup.zip', mimeType: 'application/zip', buffer: bytes});
  await reload; await page.waitForFunction(() => globalThis.lightcraft?.command);
  await page.locator('#lightcraft_loading').waitFor({state: 'hidden'});
  await selectFixture(page); await checkExport(page);
}

try {
  await workerFailure('constructor');
  await workerFailure('init');
  await lockFailure('throw');
  await lockFailure('reject');
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
    const protectedBackup = await download(page, () => page.locator('#lightcraft_recovery_backup').click());
    assert.ok(protectedBackup.length > 0);
    await page.waitForFunction(() => document.getElementById('lightcraft_recovery_status').textContent.includes('Library backed up:'));
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
