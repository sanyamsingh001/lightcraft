// Bounded Node coverage for the real font fixture helper; no browser, HTTP or builds.
import assert from 'node:assert/strict';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {test} from 'node:test';
import {command, until} from './font-browser-controls.mjs';

const run = promisify(execFile);

function timers(t) {
  const live = new Set();
  const fired = [];
  const set = globalThis.setTimeout;
  const clear = globalThis.clearTimeout;
  t.mock.method(globalThis, 'setTimeout', (callback, delay, ...args) => {
    const handle = set(() => {
      live.delete(handle);
      fired.push(delay);
      callback(...args);
    }, delay);
    live.add(handle);
    return handle;
  });
  t.mock.method(globalThis, 'clearTimeout', handle => {
    live.delete(handle);
    clear(handle);
  });
  // A failing assertion must also leave this test process with no recorded timers.
  t.after(() => { for (const handle of live) clear(handle); });
  return {live, fired};
}

test('a never-settling predicate reaches its referenced deadline in a standalone Node process', async () => {
  const helper = new URL('./font-browser-controls.mjs', import.meta.url).href;
  const script = `
    import assert from 'node:assert/strict';
    import {until} from ${JSON.stringify(helper)};
    await assert.rejects(until(() => new Promise(() => {}), 'never settles', 30),
      {message: 'never settles did not settle'});
    console.log('deadline observed');
  `;
  // This watchdog lives in the parent: it cannot keep an unreferenced child deadline alive.
  const {stdout} = await run(process.execPath, ['--input-type=module', '-e', script], {timeout: 2000});
  assert.equal(stdout.trim(), 'deadline observed');
});

test('an immediately successful check returns and clears its deadline', async t => {
  const observed = timers(t);
  let calls = 0;
  assert.equal(await until(() => { calls++; return true; }, 'success', 1000), undefined);
  assert.equal(calls, 1);
  assert.equal(observed.live.size, 0);
  assert.deepEqual(observed.fired, []);
});

test('a false check can become successful on the next poll without leaking timers', async t => {
  const observed = timers(t);
  let calls = 0;
  assert.equal(await until(async () => ++calls === 2, 'eventual success', 1000), undefined);
  assert.equal(calls, 2);
  assert.equal(observed.live.size, 0);
  assert.deepEqual(observed.fired, [100]);
});

test('a synchronous predicate error keeps its identity and clears the deadline', async t => {
  const observed = timers(t);
  const error = new Error('command rejected synchronously');
  await assert.rejects(until(() => { throw error; }, 'rejection', 1000), actual => actual === error);
  assert.equal(observed.live.size, 0);
  assert.deepEqual(observed.fired, []);
});

test('asynchronous Error and string command rejections pass through unchanged', async t => {
  const observed = timers(t);
  for (const error of [new Error('command rejected asynchronously'), 'raw command rejection']) {
    await assert.rejects(until(() => Promise.reject(error), 'rejection', 1000), actual => actual === error);
    assert.equal(observed.live.size, 0);
  }
  assert.deepEqual(observed.fired, []);
});

test('deadline expiry clears the outstanding poll timer', async t => {
  const observed = timers(t);
  let calls = 0;
  await assert.rejects(until(() => { calls++; return false; }, 'poll deadline', 30),
    {message: 'poll deadline did not settle'});
  assert.equal(calls, 1);
  assert.equal(observed.live.size, 0);
  assert.equal(observed.fired.includes(100), false);
});

test('an already expired deadline skips the predicate and clears its timer', async t => {
  const observed = timers(t);
  let calls = 0;
  await assert.rejects(until(() => { calls++; return true; }, 'expired', 0),
    {message: 'expired did not settle'});
  assert.equal(calls, 0);
  assert.equal(observed.live.size, 0);
  assert.deepEqual(observed.fired, []);
});


test('a font command has its own deadline when the app never replies', async t => {
  const observed = timers(t);
  let release, watchdog;
  const pending = command({evaluate: () => new Promise(resolve => {release = resolve;})}, 'web.stats', {}, 30);
  try {
    const result = await Promise.race([
      pending.then(value => ({value}), error => ({error})),
      new Promise(resolve => {watchdog = setTimeout(() => resolve({watchdog: true}), 300);}),
    ]);
    assert.equal(result.watchdog, undefined, 'the command must reject before the independent watchdog');
    assert.equal(result.error?.message, 'web.stats command did not settle');
  } finally {
    clearTimeout(watchdog);
    release({late: true});
    await pending.catch(() => {});
  }
  assert.equal(observed.live.size, 0);
});

test('font command success preserves the result and parameters and clears its timer', async t => {
  const observed = timers(t), result = {uiFont: {state: 'ready'}}, params = {text: 'glyphs'};
  const target = {evaluate: async (_fn, args) => {
    assert.deepEqual(args, {id: 'web.fonts.glyphs', params});
    return result;
  }};
  assert.equal(await command(target, 'web.fonts.glyphs', params), result);
  assert.equal(observed.live.size, 0);
  assert.deepEqual(observed.fired, []);
});

test('font command failures preserve thrown values and clear their timer', async t => {
  const observed = timers(t);
  for (const error of [new Error('font command refusal'), 'raw font command refusal']) {
    for (const evaluate of [() => {throw error;}, () => Promise.reject(error)]) {
      await assert.rejects(async () => command({evaluate}, 'web.stats'), actual => actual === error);
      assert.equal(observed.live.size, 0);
    }
  }
  assert.deepEqual(observed.fired, []);
});
