// Parent-run actual Chromium acceptance against an optimized, already built local bundle.
// No builds, downloads to source, credentials, or external network are required.
import assert from 'node:assert/strict';
import {readFile, mkdir, writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {fileURLToPath} from 'node:url';
import {join} from 'node:path';
import {font, verifyBundle} from './font-bundle.mjs';
import {command, until, exposeLanguageMenu, ready, installObservations, measurement, saveMeasurements, clickWidget} from './font-browser-controls.mjs';
import {deploymentCases} from './font-deployment-cases.mjs';
const bundleDirectory = process.env.LIGHTCRAFT_WEB_FONT_BUNDLE;
assert.ok(bundleDirectory, 'LIGHTCRAFT_WEB_FONT_BUNDLE must name the parent-built optimized web directory; this gate never skips it');
await verifyBundle(bundleDirectory);
const {chromium} = await import(process.env.LIGHTCRAFT_PLAYWRIGHT_MODULE || 'playwright');
const base = new URL(process.argv[2] || 'http://127.0.0.1:8080/');
assert.ok(['http:', 'https:'].includes(base.protocol) && ['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname), 'use a local fixture server');
assert.equal(base.username + base.password, '', 'fixture URL must not contain credentials');
assert.ok(base.pathname.endsWith('/'), 'fixture base URL must end in a deployment directory slash');
const output = process.argv[3] || 'target/web-font-acceptance';
await mkdir(output, {recursive: true});
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const descriptor = JSON.parse(await readFile(join(bundleDirectory, 'fonts/chinese-ui.json'), 'utf8'));
const bundleInputs = {};
let bytes;
for (const name of ['index.html', 'lightcraft_web.js', 'lightcraft_web_bg.wasm', 'worker.js',
  'fonts/chinese-ui.json', descriptor.licence, descriptor.notice, font.url]) {
  const local = await readFile(join(bundleDirectory, name));
  const response = await fetch(new URL(name, base), {signal: AbortSignal.timeout(120000)});
  assert.ok(response.ok, `baseline server serves ${name}`);
  const served = Buffer.from(await response.arrayBuffer());
  assert.equal(sha256(served), sha256(local), `baseline server must serve the exact supplied bundle: ${name}`);
  bundleInputs[name] = {bytes: local.length, sha256: sha256(local)};
  if (name === font.url) bytes = served;
}
assert.equal(bytes.length, font.bytes); assert.equal(sha256(bytes), font.sha256);
await writeFile(join(output, 'bundle-provenance.json'), JSON.stringify({base: base.href, files: bundleInputs,
  optimization: 'Parent optimized-build provenance is required; directory hashes alone do not establish the build profile.',
  zipEntryVerification: 'not performed; parent packaging gate'}, null, 2));
const browser = await chromium.launch({headless: true});
async function open(context, storage = 'memory') {
  await installObservations(context);
  const page = await context.newPage(); page.setDefaultTimeout(120000);
  const errors = []; page.on('pageerror', error => errors.push(String(error)));
  const url = new URL(base); url.search = `store=${storage}&workers=0`;
  await page.goto(url.href); await page.waitForFunction(() => globalThis.lightcraft?.command);
  await page.locator('#lightcraft_loading').waitFor({state: 'hidden'});
  await command(page, 'web.stats'); return {page, errors};
}
const click = (page, id) => clickWidget(page, page.mouse, id);
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
  const sample = async label => samples.push(await measurement(page, label));
  const held = new Promise(resolve => { release = resolve; });
  await context.route(`**/${font.url}`, async route => { requests++; await held; await route.fulfill({status: 200, contentType: 'font/otf', body: bytes}); });
  const {page, errors} = await open(context);
  assert.equal((await command(page, 'web.stats')).uiFont.state, 'idle'); assert.equal(requests, 0); await sample('before-font');
  const menuRoute = await exposeLanguageMenu(page, page.mouse);
  await until(async () => (await command(page, 'web.stats')).uiFont.state === 'loading');
  assert.equal((await command(page, 'web.stats')).uiFont.languageNames[1], 'Simplified Chinese');
  await page.screenshot({path: join(output, 'picker-pending.png')});
  await until(() => requests === 1); assert.equal(requests, 1); await sample('body-held'); release(); await ready(page, 'en'); await sample('font-installed');
  assert.equal((await command(page, 'web.stats')).uiFont.languageNames[1], '简体中文');
  // Re-expose a submenu if egui's font activation pass closed it; test its rendered endonyms.
  await exposeLanguageMenu(page, page.mouse);
  await page.screenshot({path: join(output, 'picker-endonyms.png')});
  await click(page, 'menuitem:app.language.simplifiedChinese'); await ready(page, 'zh-hans'); await glyphs(page, 'zh-hans');
  await page.screenshot({path: join(output, 'hans.png')});
  await command(page, 'app.settings'); await click(page, 'button:settingsLanguage-2');
  await ready(page, 'zh-hant'); await glyphs(page, 'zh-hant'); await page.screenshot({path: join(output, 'hant-settings.png')});
  await click(page, 'button:settingsLanguage-3'); await ready(page, 'ja'); await glyphs(page, 'ja');
  await command(page, 'app.language.simplifiedChinese'); await ready(page, 'zh-hans');
  assert.equal(requests, 1); assert.deepEqual(errors, []); await sample('after-locale-switches');
  await saveMeasurements(join(output, 'runtime-samples.json'), samples, {menuRoute, zipEntryVerification: 'not performed; parent packaging gate'});
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
  const deployments = await deploymentCases({browser, directory: bundleDirectory, output, glyphs});
  await writeFile(join(output, 'deployment-results.json'), JSON.stringify(deployments, null, 2));
  console.log('Actual Chromium font picker, saved-preference, glyph/single-flight/failure, prefix/iframe and real 30s timeout/retry assertions passed.');
} finally { await browser.close(); }
