// Real WebApp controls and observations. This module is only executed by the parent browser gate.
import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';

// Playwright's default timeout does not bound evaluate() waiting on an app command.
export async function command(target, id, params = {}, timeout = 120000) {
  let timer;
  try {
    return await Promise.race([
      Promise.resolve().then(() => target.evaluate(async ({id, params}) =>
        JSON.parse(await lightcraft.command(id, JSON.stringify(params))), {id, params})),
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(Error(`${id} command did not settle`)), timeout);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

export async function until(check, label = 'browser font condition', timeout = 120000) {
  const end = Date.now() + timeout;
  let deadlineTimer, pollTimer;
  const deadline = new Promise((_, reject) => {
    deadlineTimer = setTimeout(() => reject(Error(`${label} did not settle`)), timeout);
  });
  try {
    while (Date.now() < end) {
      if (await Promise.race([check(), deadline])) return;
      await Promise.race([new Promise(resolve => { pollTimer = setTimeout(resolve, 100); }), deadline]);
    }
    throw Error(`${label} did not settle`);
  } finally {
    clearTimeout(deadlineTimer);
    clearTimeout(pollTimer);
  }
}

async function pointer(target, mouse, id, click) {
  let rect;
  await until(async () => {
    rect = (await command(target, 'web.stats')).uiFont.pickerRects.find(widget => widget.id === id)?.rect;
    return !!rect;
  }, `widget ${id}`);
  // Fixtures use default UI zoom and device scale 1. Frame locators return top-page coordinates.
  const canvas = await target.locator('#lightcraft_canvas').boundingBox();
  assert.ok(canvas, 'the actual app canvas is visible');
  const x = canvas.x + (rect[0] + rect[2]) / 2;
  const y = canvas.y + (rect[1] + rect[3]) / 2;
  assert.ok(x >= canvas.x && x < canvas.x + canvas.width && y >= canvas.y && y < canvas.y + canvas.height,
    `${id} must be inside the actual canvas: ${rect}`);
  if (click) await mouse.click(x, y);
  else await mouse.move(x, y);
}
export const clickWidget = (target, mouse, id) => pointer(target, mouse, id, true);
export const hoverWidget = (target, mouse, id) => pointer(target, mouse, id, false);

export async function exposeLanguageMenu(target, mouse) {
  let rects;
  await until(async () => {
    rects = (await command(target, 'web.stats')).uiFont.pickerRects;
    return rects.some(widget => widget.id === 'menu:Edit' || widget.id === 'menu:all');
  }, 'direct or collapsed menu');
  const has = id => rects.some(widget => widget.id === id);
  let route;
  if (has('menusub:Language')) {
    route = 'already-exposed';
  } else if (has('menu:Edit')) {
    route = 'menu:Edit';
    await clickWidget(target, mouse, 'menu:Edit');
  } else {
    route = 'menu:all → menusub:Edit';
    assert.ok(has('menu:all'), 'collapsed menu is addressable');
    if (!has('menusub:Edit')) await clickWidget(target, mouse, 'menu:all');
    await hoverWidget(target, mouse, 'menusub:Edit');
  }
  await hoverWidget(target, mouse, 'menusub:Language');
  await until(async () => (await command(target, 'web.stats')).uiFont.pickerRects.some(widget =>
    widget.id === 'menuitem:app.language.simplifiedChinese'), 'Chinese menu entry');
  return route;
}

export async function ready(target, locale) {
  await until(async () => {
    const state = (await command(target, 'web.stats')).uiFont;
    if (state.state !== 'ready' || state.displayLanguage !== locale) return false;
    return (await command(target, 'web.fonts.glyphs', {text: '简體中文日本語'})).owned;
  }, `installed font and ${locale} display`);
}

export async function installObservations(context) {
  await context.addInitScript(() => {
    globalThis.fontAcceptance = {longTasks: [], longTaskSupported: false};
    try {
      new PerformanceObserver(list => {
        for (const entry of list.getEntries()) fontAcceptance.longTasks.push({startTime: entry.startTime, duration: entry.duration});
      }).observe({type: 'longtask', buffered: true});
      fontAcceptance.longTaskSupported = true;
    } catch { /* Record actual capability; never synthesize timings or tasks. */ }
  });
}

export async function measurement(target, label) {
  const host = await command(target, 'web.stats');
  const glyphs = host.uiFont.state === 'ready' ? await command(target, 'web.fonts.glyphs', {text: '简體中文日本語'}) : null;
  const browser = await target.evaluate(() => ({
    now: performance.now(), timeOrigin: performance.timeOrigin,
    jsHeapBytes: performance.memory?.usedJSHeapSize ?? null,
    longTaskSupported: fontAcceptance.longTaskSupported,
    longTasks: fontAcceptance.longTasks.slice(),
    fontResources: performance.getEntriesByType('resource').filter(entry =>
      new URL(entry.name).pathname.endsWith('/fonts/NotoSansCJKsc-Regular.otf')).map(entry => ({
      url: entry.name, startTime: entry.startTime, responseStart: entry.responseStart,
      responseEnd: entry.responseEnd, duration: entry.duration, transferSize: entry.transferSize,
      encodedBodySize: entry.encodedBodySize, decodedBodySize: entry.decodedBodySize,
    })),
  }));
  assert.ok(Number.isFinite(host.wasmMemoryBytes) && host.wasmMemoryBytes > 0, 'actual WASM memory size is reported');
  return {label, host, ownedFontObserved: glyphs?.owned ?? false, browser};
}

export async function saveMeasurements(file, samples, extra = {}) {
  const installed = samples.find(sample => sample.ownedFontObserved);
  const completed = installed?.browser.fontResources.filter(resource => resource.responseEnd > 0).at(-1);
  await writeFile(file, JSON.stringify({
    scope: 'actual app font resource timing and first observed egui ownership, observed app-frame WASM linear memory, optional JS heap and observed long tasks; no GPU/atlas/retained-memory proof',
    fontNetworkClock: 'Local HTTP records use Node performance.now(); browser resource/ownership observations use the app frame performance clock.',
    timingLimit: 'ownership observation is sampled after activation, not exact parser/upload instrumentation; response-end-to-observation includes validation, app frames and polling overhead',
    requestStartToOwnedObservationMs: completed ? installed.browser.now - completed.startTime : null,
    responseEndToOwnedObservationMs: completed ? installed.browser.now - completed.responseEnd : null,
    samples, ...extra,
  }, null, 2));
}
