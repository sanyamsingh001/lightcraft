import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import test from "node:test";
import {runInNewContext} from "node:vm";

// Run the production DOM recovery script without WASM or a GPU. The synthetic context-loss
// event cannot rely on anything drawn on the canvas to explain the failure or protect work.
const html = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const script = html.match(/<script id="lightcraft_graphics_recovery">([\s\S]*?)<\/script>/)?.[1];
assert.ok(script, "the recovery script must ship in the single-file browser shell");

class Element {
  hidden = false;
  disabled = false;
  textContent = "";
  listeners = new Map();
  addEventListener(event, handler) { this.listeners.set(event, handler); }
  emit(event, value = {}) { return this.listeners.get(event)?.(value); }
}

function page(command = async id => id === "library.state" ? '{"selection":{"ids":[7],"active":7}}' : "{}") {
  const elements = new Map([...html.matchAll(/id="(lightcraft_[^"]+)"/g)].map((m) => [m[1], new Element()]));
  const get = (name) => elements.get(`lightcraft_${name}`);
  assert.match(html, /id="lightcraft_recovery"[^>]* hidden/);
  assert.match(html, /#lightcraft_recovery\[hidden\]\s*\{\s*display:\s*none/);
  get("recovery").hidden = true;
  let reloads = 0;
  let confirmed = false;
  const context = {
    document: {getElementById: (id) => elements.get(id)},
    lightcraft: {command},
    confirm: () => confirmed,
    location: {reload: () => reloads++},
  };
  runInNewContext(script, context);
  return {get, context, reloads: () => reloads, confirm: (value) => { confirmed = value; }};
}

function lose(p) {
  let prevented = false;
  p.get("canvas").emit("webglcontextlost", {preventDefault: () => { prevented = true; }});
  assert.ok(prevented, "allow the browser to restore the context");
  assert.equal(p.get("recovery").hidden, false);
}

test("loss reveals recovery outside the canvas and restoration retains actionable controls", () => {
  const p = page();
  assert.equal(p.get("recovery").hidden, true);
  p.get("canvas").emit("webglcontextrestored");
  assert.equal(p.get("recovery").hidden, true);
  lose(p);
  assert.match(p.get("recovery_message").textContent, /Export the active photo/);
  p.get("canvas").emit("webglcontextrestored");
  assert.equal(p.get("recovery").hidden, false);
  assert.match(p.get("recovery_message").textContent, /Reload/);
  lose(p);
  assert.match(p.get("recovery_message").textContent, /Export the active photo/);
});

test("first loss clears stale notices while repeated loss preserves a current backup completion", async () => {
  const p = page(async () => '{"started":true}');
  const status = p.get("recovery_status");
  status.textContent = "Library backed up: 0 originals before the latest edits.";
  lose(p);
  assert.equal(status.textContent, "", "a pre-loss success must not imply that current edits are protected");
  await p.get("recovery_backup").emit("click");
  assert.match(status.textContent, /Wait for the download/);
  // The asynchronous backup service reports completion through safety::notice, including
  // a library with zero originals. A later loss must keep this current completion visible.
  const completed = "Library backed up: 0 originals (0.1 MB). Keep the file somewhere safe.";
  status.textContent = completed;
  p.get("canvas").emit("webglcontextrestored");
  lose(p);
  assert.equal(status.textContent, completed);
});

test("repeated loss preserves an in-progress action and its eventual result", async () => {
  let finish;
  let exportStarted;
  const started = new Promise(resolve => { exportStarted = resolve; });
  const p = page(async id => {
    if (id === "library.state") return '{"selection":{"ids":[7],"active":7}}';
    return new Promise(resolve => { finish = resolve; exportStarted(); });
  });
  lose(p);
  const pending = p.get("recovery_export").emit("click");
  lose(p);
  assert.equal(p.get("recovery_status").textContent, "Working…");
  assert.equal(p.get("recovery_export").disabled, true);
  await started;
  finish("{}");
  await pending;
  const completed = p.get("recovery_status").textContent;
  assert.match(completed, /Export finished/);
  lose(p);
  assert.equal(p.get("recovery_status").textContent, completed);
  assert.equal(p.get("recovery_export").disabled, false);
});

test("recovery exports and backs up through production commands while graphics are lost", async () => {
  const calls = [];
  const p = page(async (id, params) => {
    calls.push([id, JSON.parse(params)]);
    return id === "library.state" ? '{"selection":{"ids":[4,7],"active":7}}' : "{}";
  });
  lose(p);
  await p.get("recovery_export").emit("click");
  assert.deepEqual(calls[0], ["library.state", {}]);
  assert.deepEqual(calls[1], ["app.export", {format: "png", path: "/exports/LightCraft.png", ids: [7]}]);
  assert.match(p.get("recovery_status").textContent, /Export finished/);
  await p.get("recovery_backup").emit("click");
  assert.deepEqual(calls[2], ["file.backupLibrary", {}]);
  assert.match(p.get("recovery_status").textContent, /Wait for the download/);
  assert.equal(p.get("recovery_backup").disabled, false);
});

test("recovery exports only the active photo when another selected original is unavailable", async () => {
  const calls = [];
  const p = page(async (id, params) => {
    const args = JSON.parse(params);
    calls.push([id, args]);
    if (id === "library.state") return '{"selection":{"ids":[41,42],"active":42}}';
    if (id === "app.export") {
      // The command's default selection includes the unavailable original; explicit B works.
      const ids = args.ids ?? [41, 42];
      if (ids.includes(41)) throw new Error("inactive original unavailable");
      return '{"files":[{"path":"/exports/LightCraft.png"}]}';
    }
    throw new Error(`unexpected command: ${id}`);
  });
  lose(p);
  await p.get("recovery_export").emit("click");
  assert.deepEqual(calls, [
    ["library.state", {}],
    ["app.export", {format: "png", path: "/exports/LightCraft.png", ids: [42]}],
  ]);
  assert.match(p.get("recovery_status").textContent, /Export finished/);
  assert.match(html, /Export Active Photo as PNG/);
  assert.match(html, /the active photo's current appearance/);
});

test("each recovery export queries the current active photo", async () => {
  let active = 42;
  const exported = [];
  const p = page(async (id, params) => {
    if (id === "library.state") return JSON.stringify({selection: {ids: [41, 42], active}});
    exported.push(JSON.parse(params).ids);
    return "{}";
  });
  lose(p);
  await p.get("recovery_export").emit("click");
  active = 41;
  await p.get("recovery_export").emit("click");
  assert.deepEqual(exported, [[42], [41]]);
});

test("no active photo reports a visible error without exporting the selection", async () => {
  for (const ids of [[], [41, 42]]) {
    const calls = [];
    const p = page(async id => {
      calls.push(id);
      return JSON.stringify({selection: {ids, active: null}});
    });
    lose(p);
    await p.get("recovery_export").emit("click");
    assert.deepEqual(calls, ["library.state"]);
    assert.match(p.get("recovery_status").textContent, /No active photo to export/);
    assert.equal(p.get("recovery_export").disabled, false);
  }
});

test("active-photo lookup failure stays visible and can be retried", async () => {
  let failed = true;
  let exported = 0;
  const p = page(async id => {
    if (id === "library.state") {
      if (failed) throw new Error("Active photo lookup failed");
      return '{"selection":{"ids":[7],"active":7}}';
    }
    exported++;
    return "{}";
  });
  lose(p);
  await p.get("recovery_export").emit("click");
  assert.equal(exported, 0);
  assert.match(p.get("recovery_status").textContent, /Active photo lookup failed/);
  assert.equal(p.get("recovery_export").disabled, false);
  failed = false;
  await p.get("recovery_export").emit("click");
  assert.equal(exported, 1);
  assert.match(p.get("recovery_status").textContent, /Export finished/);
});

test("failed protection actions remain visible and retryable", async () => {
  const p = page(async () => { throw new Error("Browser storage refused the backup"); });
  lose(p);
  await p.get("recovery_backup").emit("click");
  assert.match(p.get("recovery_status").textContent, /storage refused/);
  assert.equal(p.get("recovery_backup").disabled, false);
  delete p.context.lightcraft;
  await p.get("recovery_export").emit("click");
  assert.match(p.get("recovery_status").textContent, /still starting/);
  assert.equal(p.get("recovery_export").disabled, false);
});

test("reload waits for explicit confirmation so memory-only work can be protected", () => {
  const p = page();
  lose(p);
  p.get("recovery_reload").emit("click");
  assert.equal(p.reloads(), 0);
  p.confirm(true);
  p.get("recovery_reload").emit("click");
  assert.equal(p.reloads(), 1);
});


test("recovery rejects an active ID that JSON numbers cannot preserve", async () => {
  for (const active of [Number.MAX_SAFE_INTEGER + 1, -1, "7"]) {
    const calls = [];
    const p = page(async (id, params) => {
      calls.push(id);
      return id === "library.state" ? JSON.stringify({selection: {active}}) : "{}";
    });
    lose(p);
    await p.get("recovery_export").emit("click");
    assert.deepEqual(calls, ["library.state"]);
    assert.match(p.get("recovery_status").textContent, /cannot be exported safely/);
    assert.equal(p.get("recovery_export").disabled, false);
  }
});
