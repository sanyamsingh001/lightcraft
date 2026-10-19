// Parent-run actual Chromium acceptance against an optimized, already built local bundle.
// No builds, downloads to source, credentials, or external network are required.
import assert from 'node:assert/strict';
import {readFile, mkdir, writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {fileURLToPath} from 'node:url';
import {join} from 'node:path';
import {font, verifyBundle} from './font-bundle.mjs';
const {chromium} = await import(process.env.LIGHTCRAFT_PLAYWRIGHT_MODULE || 'playwright');
const base = new URL(process.argv[2] || 'http://127.0.0.1:8080/');
assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname), 'use a local fixture server');
const output = process.argv[3] || 'target/web-font-acceptance';
await mkdir(output, {recursive: true});
if (process.env.LIGHTCRAFT_WEB_FONT_BUNDLE) await verifyBundle(process.env.LIGHTCRAFT_WEB_FONT_BUNDLE);
const response = await fetch(new URL(font.url, base)); assert.ok(response.ok);
const bytes = Buffer.from(await response.arrayBuffer());
assert.equal(bytes.length, font.bytes); assert.equal(createHash('sha256').update(bytes).digest('hex'), font.sha256);
const browser = await chromium.launch({headless: true});
const command = (page, id, params = {}) => page.evaluate(async ({id, params}) => JSON.parse(await lightcraft.command(id, JSON.stringify(params))), {id, params});
async function until(check) {
  const end = Date.now() + 120000;
  while (Date.now() < end) { if (await check()) return; await new Promise(r => setTimeout(r, 100)); }
  throw Error('font acceptance did not settle');
}
async function open(context, storage = 'memory') {
  await context.addInitScript(() => {
    globalThis.fontAcceptanceLongTasks = [];
    try { new PerformanceObserver(list => { for (const entry of list.getEntries()) fontAcceptanceLongTasks.push({startTime: entry.startTime, duration: entry.duration}); }).observe({type: 'longtask', buffered: true}); } catch { /* Chrome capability is recorded, not assumed. */ }
  });
  const page = await context.newPage(); page.setDefaultTimeout(120000);
  const errors = []; page.on('pageerror', e => errors.push(String(e)));
  const url = new URL(base); url.search = `store=${storage}&workers=0`;
  await page.goto(url.href); await page.waitForFunction(() => globalThis.lightcraft?.command);
  await page.locator('#lightcraft_loading').waitFor({state: 'hidden'});
  await command(page, 'web.stats'); return {page, errors};
}
async function click(page, id) {
  let rect;
  await until(async () => { rect = (await command(page, 'web.stats')).uiFont.pickerRects.find(w => w.id === id)?.rect; return !!rect; });
  // Default UI zoom/device scale in this fixture is 1. Native point conversion has separate gates.
  const canvas = await page.locator('#lightcraft_canvas').boundingBox(); assert.ok(canvas);
  await page.mouse.click(canvas.x + (rect[0] + rect[2]) / 2, canvas.y + (rect[1] + rect[3]) / 2);
}
async function hover(page, id) {
  let rect;
  await until(async () => { rect = (await command(page, 'web.stats')).uiFont.pickerRects.find(w => w.id === id)?.rect; return !!rect; });
  const canvas = await page.locator('#lightcraft_canvas').boundingBox(); assert.ok(canvas);
  await page.mouse.move(canvas.x + (rect[0] + rect[2]) / 2, canvas.y + (rect[1] + rect[3]) / 2);
}
async function ready(page, locale) {
  await until(async () => {
    const s = (await command(page, 'web.stats')).uiFont;
    if (s.state !== 'ready' || s.displayLanguage !== locale) return false;
    return (await command(page, 'web.fonts.glyphs', {text: '简體中文日本語'})).owned;
  });
}
async function glyphs(page, code) {
  const directory = new URL('../../../crates/ui-egui/locales/', import.meta.url);
  const chars = new Set('简体中文繁體中文（台灣）日本語');
  for (const name of [`${code}.json`, `${code}-formats.json`]) {
    const catalog = JSON.parse(await readFile(fileURLToPath(new URL(name, directory)), 'utf8'));
    for (const text of Object.values(catalog)) for (const char of text) chars.add(char);
  }
  const text = [...chars].join('');
  const result = await command(page, 'web.fonts.glyphs', {text});
  assert.equal(result.missing[0], ''); assert.equal(result.missing[1], ''); assert.equal(result.owned, true);
  // Monospace is for Han/kana in this font coverage gate, not every symbol in body translations.
  const cjk = text.replace(/[^\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}]/gu, '');
  assert.deepEqual((await command(page, 'web.fonts.glyphs', {text: cjk})).missing, ['', '', '']);
  for (const names of result.familyOrder) {
    assert.ok(!names[0].startsWith('craft-fonts'));
    const cjk = names.find(n => n.startsWith('craft-fonts'));
    assert.ok(cjk.includes(code === 'ja' ? 'BIZ UDPGothic' : 'Noto Sans CJK SC'));
  }
}
try {
  // Picker exposure while English: defer one request, retain readability, then render endonyms.
  const context = await browser.newContext({viewport: {width: 1100, height: 800}, deviceScaleFactor: 1});
  let release; let requests = 0; const samples = [];
  const sample = async label => samples.push({label, host: await command(page, 'web.stats'), browser: await page.evaluate(() => ({now: performance.now(), jsHeapBytes: performance.memory?.usedJSHeapSize ?? null, longTasks: fontAcceptanceLongTasks}))});
  const held = new Promise(resolve => { release = resolve; });
  await context.route(`**/${font.url}`, async route => { requests++; await held; await route.fulfill({status: 200, contentType: 'font/otf', body: bytes}); });
  const {page, errors} = await open(context);
  assert.equal((await command(page, 'web.stats')).uiFont.state, 'idle'); assert.equal(requests, 0); await sample('before-font');
  await click(page, 'menu:Edit'); await hover(page, 'menusub:Language');
  await until(async () => (await command(page, 'web.stats')).uiFont.state === 'loading');
  assert.equal((await command(page, 'web.stats')).uiFont.languageNames[1], 'Simplified Chinese');
  await page.screenshot({path: join(output, 'picker-pending.png')});
  await until(() => requests === 1); assert.equal(requests, 1); await sample('body-held'); release(); await ready(page, 'en'); await sample('font-installed');
  assert.equal((await command(page, 'web.stats')).uiFont.languageNames[1], '简体中文');
  // Re-expose a submenu if egui's font activation pass closed it; test its rendered endonyms.
  if (!(await command(page, 'web.stats')).uiFont.pickerRects.some(w => w.id === 'menusub:Language')) await click(page, 'menu:Edit');
  await hover(page, 'menusub:Language');
  await page.screenshot({path: join(output, 'picker-endonyms.png')});
  await click(page, 'menuitem:app.language.simplifiedChinese'); await ready(page, 'zh-hans'); await glyphs(page, 'zh-hans');
  await page.screenshot({path: join(output, 'hans.png')});
  await command(page, 'app.settings'); await click(page, 'button:settingsLanguage-2');
  await ready(page, 'zh-hant'); await glyphs(page, 'zh-hant'); await page.screenshot({path: join(output, 'hant-settings.png')});
  await click(page, 'button:settingsLanguage-3'); await ready(page, 'ja'); await glyphs(page, 'ja');
  await command(page, 'app.language.simplifiedChinese'); await ready(page, 'zh-hans');
  assert.equal(requests, 1); assert.deepEqual(errors, []); await sample('after-locale-switches');
  await writeFile(join(output, 'runtime-samples.json'), JSON.stringify({scope: 'actual Chromium main-page WASM linear memory, JS heap if supported and long tasks; excludes GPU/atlas retained-memory proof', samples}, null, 2));
  await context.close();
  // Saved startup requests Chinese, then a delayed result follows the latest selected locale.
  for (const locale of ['zh-hans', 'zh-hant']) {
    const context = await browser.newContext({viewport: {width: 1100, height: 800}, deviceScaleFactor: 1});
    let hold = false, resume;
    await context.route(`**/${font.url}`, async route => { if (hold) await new Promise(r => { resume = r; }); await route.fulfill({status: 200, contentType: 'font/otf', body: bytes}); });
    const {page, errors} = await open(context, 'idb');
    await command(page, locale === 'zh-hans' ? 'app.language.simplifiedChinese' : 'app.language.traditionalChinese');
    await ready(page, locale);
    await until(async () => { const s = await command(page, 'web.stats'); return !s.dirty && !s.flushing; });
    // Wait for a persisted UI snapshot, then force a cold-font page instance in the same context.
    await page.waitForTimeout(1200); hold = true; await page.reload();
    await page.waitForFunction(() => globalThis.lightcraft?.command);
    await until(async () => (await command(page, 'web.stats')).uiFont.state === 'loading');
    const pending = (await command(page, 'web.stats')).uiFont;
    assert.equal(pending.preferredLanguage, locale); assert.equal(pending.displayLanguage, 'en');
    await command(page, 'app.language.japanese'); await until(async () => (await command(page, 'web.stats')).uiFont.displayLanguage === 'ja');
    await until(() => typeof resume === 'function'); resume(); await ready(page, 'ja'); await glyphs(page, 'ja');
    assert.equal((await command(page, 'web.stats')).uiFont.requests, 1); assert.deepEqual(errors, []);
    await context.close();
  }
  // Failures stay usable, preserve preference and need an explicit same-choice Settings retry.
  for (const failure of ['404', 'truncated', 'oversize', 'hash']) {
    const context = await browser.newContext({viewport: {width: 1100, height: 800}, deviceScaleFactor: 1});
    let requests = 0;
    await context.route(`**/${font.url}`, async route => {
      requests++;
      const body = requests > 1 ? bytes : failure === 'truncated' ? bytes.subarray(0, 100) : failure === 'oversize' ? Buffer.concat([bytes, Buffer.from([0])]) : failure === 'hash' ? Buffer.alloc(bytes.length) : Buffer.alloc(0);
      await route.fulfill({status: requests === 1 && failure === '404' ? 404 : 200, contentType: 'font/otf', body});
    });
    const {page, errors} = await open(context); await command(page, 'app.language.simplifiedChinese');
    await until(async () => (await command(page, 'web.stats')).uiFont.state === 'failed');
    const failed = await command(page, 'web.stats'); assert.equal(failed.uiFont.preferredLanguage, 'zh-hans'); assert.equal(failed.uiFont.displayLanguage, 'en');
    assert.match(failed.toast, /Chinese text could not be loaded:.*retry/); assert.ok(failed.uiFont.error);
    await command(page, 'app.settings'); await page.waitForTimeout(500); assert.equal(requests, 1);
    await click(page, 'button:settingsLanguage-1'); await ready(page, 'zh-hans'); await glyphs(page, 'zh-hans');
    assert.equal(requests, 2); assert.deepEqual(errors, []); await context.close();
  }
  console.log('Actual Chromium font glyph, picker, saved-preference, single-flight and failure/retry assertions passed.');
} finally { await browser.close(); }
