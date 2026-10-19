// Parent-executed real app cases. Every deployment serves the supplied generated bundle unchanged.
import assert from 'node:assert/strict';
import {join} from 'node:path';
import {command, until, clickWidget, exposeLanguageMenu, ready, installObservations,
  measurement, saveMeasurements} from './font-browser-controls.mjs';
import {fixtureServer} from './font-fixture-server.mjs';
import {font} from './font-bundle.mjs';

async function open(browser, server, mount, embedded) {
  const context = await browser.newContext({viewport: {width: 1100, height: 800}, deviceScaleFactor: 1});
  try {
    await installObservations(context);
    const page = await context.newPage(); page.setDefaultTimeout(120000);
    const errors = []; page.on('pageerror', error => errors.push(String(error)));
    const address = embedded ? server.iframe(`${mount.url.pathname.split('/').filter(Boolean).at(-1)}-host`, mount) : mount.url;
    await page.goto(address.href);
    let target = page;
    if (embedded) {
      const iframe = await page.locator('#font_app').elementHandle(); assert.ok(iframe);
      target = await iframe.contentFrame(); assert.ok(target, 'real iframe browsing context');
    }
    await target.waitForFunction(() => globalThis.lightcraft?.command);
    await target.locator('#lightcraft_loading').waitFor({state: 'hidden'});
    assert.equal(new URL(target.url()).origin, server.origin, 'actual app and font asset use the local same origin');
    const state = await command(target, 'web.stats');
    assert.equal(state.storage, 'memory'); assert.equal(state.uiFont.state, 'idle');
    assert.equal(mount.requests.length, 0, 'font is deferred before picker/Chinese exposure');
    return {context, page, target, errors};
  } catch (error) { await context.close(); throw error; }
}
function requests(mount, count) {
  assert.equal(mount.requests.length, count);
  for (const request of mount.requests) assert.equal(request.url, new URL(font.url, mount.url).href,
    'actual browser font fetch preserves the deployment prefix and same origin');
}

export async function deploymentCases({browser, directory, output, glyphs}) {
  const server = await fixtureServer(directory);
  const results = [];
  try {
    for (const embedded of [false, true]) {
      const label = embedded ? 'iframe' : 'prefix';
      const mount = server.mount(`${label}-latest`, ['hold']);
      const {context, page, target, errors} = await open(browser, server, mount, embedded);
      const samples = [];
      try {
        samples.push(await measurement(target, 'before-font'));
        const menuRoute = await exposeLanguageMenu(target, page.mouse);
        await until(() => mount.requests.length === 1, `${label} actual held font request`);
        await until(async () => (await command(target, 'web.stats')).uiFont.state === 'loading');
        assert.equal((await command(target, 'web.stats')).uiFont.languageNames[1], 'Simplified Chinese');
        samples.push(await measurement(target, 'held-body'));
        // Preserve the latest preference while the real request is pending, with no second flight.
        await command(target, 'app.language.simplifiedChinese');
        await command(target, 'app.language.traditionalChinese');
        assert.equal((await command(target, 'web.stats')).uiFont.preferredLanguage, 'zh-hant');
        assert.equal((await command(target, 'web.stats')).uiFont.displayLanguage, 'en');
        await command(target, 'app.language.japanese');
        assert.equal((await command(target, 'web.stats')).uiFont.displayLanguage, 'ja');
        requests(mount, 1); mount.release();
        await ready(target, 'ja');
        samples.push(await measurement(target, 'first-owned-font'));
        await glyphs(target, 'ja');
        // Read the endonyms before choosing Chinese in the real menu, including collapsed menus.
        await exposeLanguageMenu(target, page.mouse);
        assert.equal((await command(target, 'web.stats')).uiFont.languageNames[1], '简体中文');
        await page.screenshot({path: join(output, `${label}-endonyms.png`)});
        await clickWidget(target, page.mouse, 'menuitem:app.language.simplifiedChinese');
        await ready(target, 'zh-hans'); await glyphs(target, 'zh-hans');
        await command(target, 'app.settings');
        await clickWidget(target, page.mouse, 'button:settingsLanguage-2');
        await ready(target, 'zh-hant'); await glyphs(target, 'zh-hant');
        samples.push(await measurement(target, 'after-menu-settings'));
        await page.screenshot({path: join(output, `${label}-hant-settings.png`)});
        requests(mount, 1); assert.equal((await command(target, 'web.stats')).uiFont.requests, 1);
        assert.deepEqual(errors, []);
        await saveMeasurements(join(output, `${label}-runtime.json`), samples, {menuRoute, deployment: target.url(),
          fontNetwork: mount.requests, zipEntryVerification: 'not performed; parent packaging gate'});
        results.push({case: `${label}-latest-glyphs`, menuRoute, fontRequests: mount.requests.length});
      } finally { mount.release(); await context.close(); }

      const failure = server.mount(`${label}-error`, ['404', 'ok']);
      const failedApp = await open(browser, server, failure, embedded);
      try {
        await command(failedApp.target, 'app.language.traditionalChinese');
        await until(async () => (await command(failedApp.target, 'web.stats')).uiFont.state === 'failed');
        const state = await command(failedApp.target, 'web.stats');
        assert.equal(state.uiFont.preferredLanguage, 'zh-hant'); assert.equal(state.uiFont.displayLanguage, 'en');
        assert.match(state.toast, /Chinese text could not be loaded:.*HTTP 404.*retry/);
        assert.ok(await command(failedApp.target, 'library.info'), 'real app commands remain usable');
        await command(failedApp.target, 'app.settings');
        await failedApp.page.waitForTimeout(500); requests(failure, 1);
        await clickWidget(failedApp.target, failedApp.page.mouse, 'button:settingsLanguage-2');
        await ready(failedApp.target, 'zh-hant'); await glyphs(failedApp.target, 'zh-hant');
        requests(failure, 2); assert.equal((await command(failedApp.target, 'web.stats')).uiFont.requests, 2);
        assert.deepEqual(failedApp.errors, []);
        results.push({case: `${label}-404-explicit-current-choice-retry`, fontNetwork: failure.requests});
      } finally { await failedApp.context.close(); }
    }

    const timeout = server.mount('prefix-timeout', ['timeout', 'ok']);
    const app = await open(browser, server, timeout, false);
    const samples = [];
    try {
      samples.push(await measurement(app.target, 'before-timeout-request'));
      const started = await app.target.evaluate(() => performance.now());
      await command(app.target, 'app.language.simplifiedChinese');
      await until(() => timeout.requests.length === 1 && timeout.requests[0].partialBytes === 65536, 'real partial font body');
      await until(async () => (await command(app.target, 'web.stats')).uiFont.state === 'loading');
      // No route.fulfill, clock manipulation, forced rejection or manual release of the first body.
      // This wait exercises the production AbortController's real 30,000 ms deadline.
      await until(async () => (await command(app.target, 'web.stats')).uiFont.state === 'failed', 'real 30s font timeout');
      const elapsedMs = await app.target.evaluate(start => performance.now() - start, started);
      assert.ok(elapsedMs >= 30000, `real elapsed timeout observation must be at least 30s: ${elapsedMs}`);
      await until(() => timeout.requests[0].closedWithoutResponse, 'browser abort closes the stalled HTTP response');
      const state = await command(app.target, 'web.stats');
      assert.equal(state.uiFont.preferredLanguage, 'zh-hans'); assert.equal(state.uiFont.displayLanguage, 'en');
      assert.match(state.uiFont.error, /timed out|request failed/i);
      assert.match(state.toast, /Chinese text could not be loaded:.*retry/);
      samples.push(await measurement(app.target, 'after-real-timeout'));
      assert.ok(await command(app.target, 'library.info'));
      await command(app.target, 'app.settings'); await app.page.waitForTimeout(500); requests(timeout, 1);
      await clickWidget(app.target, app.page.mouse, 'button:settingsLanguage-1');
      await ready(app.target, 'zh-hans'); await glyphs(app.target, 'zh-hans');
      requests(timeout, 2); assert.equal((await command(app.target, 'web.stats')).uiFont.requests, 2);
      assert.deepEqual(app.errors, []);
      samples.push(await measurement(app.target, 'after-explicit-timeout-retry'));
      await saveMeasurements(join(output, 'timeout-runtime.json'), samples, {realElapsedMs: elapsedMs, fontNetwork: timeout.requests});
      results.push({case: 'real-30s-body-timeout-explicit-retry', elapsedMs, fontNetwork: timeout.requests});
    } finally { await app.context.close(); }
    assert.deepEqual(server.failures, [], 'local fixture server had no unexplained failures');
    return results;
  } finally { await server.close(); }
}
