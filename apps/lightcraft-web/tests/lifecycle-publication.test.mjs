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
for (const [fallback, terminal] of [[false, false], [true, false], [false, true]]) {
  test(`WEB-only command boundary ${fallback}/${terminal}`, async () => {
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
          workers: {alive: 0, ready: 0, remoteJobs: 0, inlineJobs: 1},
        };
        return {};
      },
    };
    runInNewContext(helper, env);
    await env.inlineReloadRendering(fallback, terminal);
    assert.ok(calls.includes('view.grid') && calls.includes('library.select') && calls.includes('web.stats'));
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
