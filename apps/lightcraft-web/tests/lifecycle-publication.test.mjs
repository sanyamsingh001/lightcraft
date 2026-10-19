import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import test from 'node:test';
import {runInNewContext} from 'node:vm';
import {startThumbnailPlan} from './thumbnail-lifecycle.mjs';

// Exercise the shipped helper's command boundary with browser/renderer facades. Actual IDB,
// WASM and Grid rendering are covered only by the parent-run browser-lifecycle harness.
const source = readFileSync(new URL('./browser-lifecycle.mjs', import.meta.url), 'utf8');
const start = source.indexOf('async function inlineReloadRendering(');
const end = source.indexOf('async function lockFailure(', start);
assert.ok(start >= 0 && end > start);
const helper = source.slice(start, end);
for (const [fallback, terminal, missingSelected = false] of [[false, false], [true, false], [false, true], [false, false, true]]) {
  test(`WEB-only command boundary ${fallback}/${terminal}${missingSelected ? " demo-only textures" : ""}`, async () => {
    const calls = [];
    let closed = 0, reloaded = false, released = false;
    const page = {
      evaluate: async fn => {
        const body = fn.toString();
        if (body.includes('originalReadCallbacks.length')) return 1;
        if (body.includes('originalReadCallbacks.splice')) released = true;
        if (body.includes('originalReads')) return ['originals/healthy-fixture'];
      },
      goto: async () => { reloaded = true; }, reload: async () => {}, waitForFunction: async () => {},
      locator: () => ({waitFor: async () => {}, textContent: async () => 'fixture original read refusal'}),
    };
    const context = {addInitScript: async () => {}, route: async () => {}, close: async () => { closed++; }};
    const env = {
      assert, URL, console: {log() {}}, base: new URL('http://127.0.0.1:8080/'),
      browser: {newContext: async () => context}, opened: async () => page,
      importFixture: async () => ({id: 1}), selectFixture: async () => ({id: 1}), checkExport: async () => {},
      until: async check => assert.equal(Boolean(await check()), true),
      command: async (_page, id) => {
        calls.push(id);
        assert.notEqual(id, 'library.buildThumbnails', 'WEB-only must use existing rendering');
        assert.notEqual(id, 'library.buildPreviews', 'a synchronous preview build cannot test readiness');
        assert.notEqual(id, 'library.previewProgress', 'WEB-only must not need thumbnail progress');
        if (id === 'web.stats') return {
          dirty: false, flushing: false, originalsInMemory: 0,
          renderQueue: reloaded && !terminal && !released ? 1 : 0, rendersInFlight: 0, thumbTextures: 1,
          activeThumbnailReady: reloaded && !terminal && released && !missingSelected,
          workers: {alive: 0, ready: 0, remoteJobs: 0, inlineJobs: 1},
        };
        return {};
      },
    };
    runInNewContext(helper, env);
    if (missingSelected) await assert.rejects(env.inlineReloadRendering(fallback, terminal), /selected healthy fixture must have a rendered thumbnail/);
    else await env.inlineReloadRendering(fallback, terminal);
    assert.ok(calls.includes('view.photoGrid') && calls.includes('library.select') && calls.includes('web.stats'));
    assert.equal(closed, 1);
  });
}

for (const terminal of [false, true]) {
  test(`explicit thumbnail extension retains progress assertions ${terminal}`, async () => {
    const calls = [];
    const command = async (_page, id, params) => {
      calls.push({id, params});
      if (id === 'library.buildThumbnails') return {};
      assert.equal(id, 'library.previewProgress');
      return {total: 1, done: 1, failed: terminal ? 1 : 0, running: false, skipped: 0, what: 'thumbnails', error: null};
    };
    const plan = await startThumbnailPlan({page: {}, command, until: async check => assert.ok(await check())});
    await plan.complete(terminal);
    assert.deepEqual(calls[0], {id: 'library.buildThumbnails', params: {wait: false}});
  });
}

test('thumbnail extension propagates real command and progress errors', async () => {
  await assert.rejects(startThumbnailPlan({page: {}, command: async () => { throw Error('fixture command refusal'); }}), /fixture command refusal/);
  const plan = await startThumbnailPlan({
    page: {}, command: async (_page, id) => {
      if (id === 'library.buildThumbnails') return {};
      throw Error('fixture progress refusal');
    }, until: async check => check(),
  });
  await assert.rejects(plan.held(), /fixture progress refusal/);
  await assert.rejects(plan.complete(false), /fixture progress refusal/);
});

// Evaluate the shipped backing-write initializer with IDB/sessionStorage facades. A new page
// gets a new prototype, while sessionStorage survives reload inside this isolated context.
test('startup backing-write recovery remains disabled after reload', () => {
  const initializer = source.match(/async function backingSaveWarning[\s\S]*?await context\.addInitScript\((startup => \{[\s\S]*?\n    \}), startup\)/)?.[1];
  assert.ok(initializer);
  const storage = new Map();
  const sessionStorage = {getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value)};
  const writes = [];
  const page = () => {
    class IDBObjectStore { name = 'files'; put(value, key) { writes.push({value, key}); } }
    const env = {IDBObjectStore, sessionStorage, DOMException};
    runInNewContext(`(${initializer})(true)`, env);
    return {env, files: new IDBObjectStore()};
  };
  const first = page();
  assert.throws(() => first.files.put('healthy snapshot', 'library/catalog.snap'), /fixture backing write refusal/);
  first.env.refuseCatalogWrites = false;
  sessionStorage.setItem('backing-write-recovered', 'yes');
  first.files.put('healthy snapshot', 'library/catalog.snap');
  const reloaded = page();
  assert.equal(reloaded.env.refuseCatalogWrites, false);
  reloaded.files.put('healthy recovered edit', 'library/catalog.log');
  assert.equal(writes.length, 2);
});
