// Executes the actual lifecycle helper definitions with explicit page/context facades.
// No Playwright import, browser, WASM, storage, or HTTP. Independent watchdogs bound failures.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
import test from 'node:test';

function controls() {
  const source = readFileSync(new URL('./browser-lifecycle.mjs', import.meta.url), 'utf8');
  const deadline = source.indexOf('async function deadline(');
  const start = deadline >= 0 ? deadline : source.indexOf('const command =');
  const end = source.indexOf('async function download(', start);
  assert.ok(start >= 0 && end > start);
  const timers = new Set();
  const env = {assert, URL, Date, base: new URL('http://127.0.0.1:8080/'),
    setTimeout(callback, delay) {
      const timer = setTimeout(() => {timers.delete(timer); callback();}, delay);
      timers.add(timer); return timer;
    },
    clearTimeout(timer) {timers.delete(timer); clearTimeout(timer);},
  };
  runInNewContext(source.slice(start, end) + '\nglobalThis.subject = {command, until, opened};', env);
  return {...env.subject, timers};
}
async function observed(promise, timeout = 200) {
  let timer;
  try {
    return await Promise.race([promise.then(value => ({value}), error => ({error})),
      new Promise(resolve => {timer = setTimeout(() => resolve({watchdog: true}), timeout);})]);
  } finally {clearTimeout(timer);}
}

test('a pending lifecycle predicate rejects at its deadline and clears its timers', async () => {
  const c = controls(); let release;
  const held = new Promise(resolve => {release = resolve;});
  const pending = c.until(() => held, 30);
  const result = await observed(pending);
  release(false); await pending.catch(() => {});
  assert.match(String(result.error), /browser condition did not settle/);
  assert.equal(result.watchdog, undefined);
  assert.equal(c.timers.size, 0);
});

test('undrained lifecycle commands have an independent deadline', async () => {
  const c = controls(); let release;
  const page = {evaluate: () => new Promise(resolve => {release = resolve;})};
  const pending = c.command(page, 'web.stats', {}, 30);
  const result = await observed(pending);
  release('{}'); await pending.catch(() => {});
  assert.match(String(result.error), /web.stats.*did not settle/);
  assert.equal(result.watchdog, undefined);
  assert.equal(c.timers.size, 0);
});

function startupPage(blocked, commandHeld = true) {
  const calls = []; let release;
  const page = {
    setDefaultTimeout() {}, setDefaultNavigationTimeout() {},
    goto: async () => {calls.push('navigate');},
    waitForFunction: async () => {calls.push('API exposed');},
    locator: () => ({waitFor: async () => {}, count: async () => blocked ? 1 : 0,
      textContent: async () => blocked ? 'LightCraft could not read the library: fixture startup refusal' : ''}),
    evaluate: async (fn, args) => {
      if (fn.toString().includes('JSON.parse(await lightcraft.command')) {
        calls.push('command');
        return commandHeld ? new Promise(resolve => {release = resolve;}) : {storage: 'idb'};
      }
      calls.push('startup state');
      return runInNewContext('(' + fn.toString() + ')(args)', {args,
        lightcraft: {command() {}}, document: {getElementById(id) {
          if (id === 'lightcraft_blocking' && blocked) return {textContent: 'LightCraft could not read the library: fixture startup refusal'};
          return null;
        }},
      });
    },
  };
  return {page, calls, release: () => release?.({storage: 'idb'})};
}

test('visible startup refusal is reported before any undrained web.stats command', async () => {
  const c = controls(), fixture = startupPage(true);
  const pending = c.opened({newPage: async () => fixture.page}, 'store=idb', 30);
  const result = await observed(pending);
  fixture.release(); await pending.catch(() => {});
  assert.match(String(result.error), /fixture startup refusal/);
  assert.equal(result.watchdog, undefined);
  assert.equal(fixture.calls.includes('command'), false);
  assert.equal(c.timers.size, 0);
});

test('startup remains bounded if creating the page itself never settles', async () => {
  const c = controls(); let release;
  const pending = c.opened({newPage: () => new Promise(resolve => {release = resolve;})}, 'store=idb', 30);
  const result = await observed(pending);
  release(startupPage(false, false).page); await pending.catch(() => {});
  assert.match(String(result.error), /browser startup.*did not settle/);
  assert.equal(result.watchdog, undefined);
  assert.equal(c.timers.size, 0);
});

test('successful startup checks readiness before web.stats and clears its timers', async () => {
  const c = controls(), fixture = startupPage(false, false);
  assert.equal(await c.opened({newPage: async () => fixture.page}, 'store=idb', 1000), fixture.page);
  assert.ok(fixture.calls.indexOf('startup state') >= 0);
  assert.ok(fixture.calls.indexOf('startup state') < fixture.calls.indexOf('command'));
  assert.equal(c.timers.size, 0);
});

test('success, predicate errors and raw command rejection keep their public result and clean timers', async () => {
  const c = controls();
  assert.equal(await c.until(() => true, 1000), undefined);
  const error = new Error('fixture predicate refusal');
  await assert.rejects(c.until(() => {throw error;}, 1000), actual => actual === error);
  await assert.rejects(c.command({evaluate: () => Promise.reject('raw command refusal')}, 'photo.rate', {}, 1000), actual => actual === 'raw command refusal');
  assert.equal(c.timers.size, 0);
});

test('false polling and a late predicate completion cannot leave acceptance timers running', async () => {
  const c = controls();
  const polling = await observed(c.until(() => false, 30), 400);
  assert.match(String(polling.error), /browser condition did not settle/);
  assert.equal(c.timers.size, 0);
  let release;
  const pending = c.until(() => new Promise(resolve => {release = resolve;}), 30);
  const result = await observed(pending);
  release(false); await pending.catch(() => {});
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.match(String(result.error), /browser condition did not settle/);
  assert.equal(result.watchdog, undefined);
  assert.equal(c.timers.size, 0);
});
