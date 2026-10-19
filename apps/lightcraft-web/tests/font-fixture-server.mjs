// Local real static-bundle server for prefix/iframe/font-body fault acceptance. No builds or mocks of the WebApp.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {createReadStream} from 'node:fs';
import {realpath, stat} from 'node:fs/promises';
import {join, relative, isAbsolute, extname} from 'node:path';
import {pipeline} from 'node:stream/promises';
import {performance} from 'node:perf_hooks';
import {font, fileLimit} from './font-bundle.mjs';

const types = {'.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
  '.wasm': 'application/wasm', '.otf': 'font/otf', '.json': 'application/json', '.txt': 'text/plain; charset=utf-8'};
function headers(response, type) {
  response.setHeader('Content-Type', type);
  response.setHeader('Cache-Control', 'no-cache');
  response.setHeader('Cross-Origin-Opener-Policy', 'same-origin');
  response.setHeader('Cross-Origin-Embedder-Policy', 'require-corp');
  response.setHeader('Cross-Origin-Resource-Policy', 'same-origin');
}

export async function fixtureServer(directory) {
  const root = await realpath(directory);
  const mounts = new Map();
  const wrappers = new Map();
  const failures = [];
  let origin;
  async function file(response, name) {
    const path = await realpath(join(root, name));
    const rel = relative(root, path);
    assert.ok(rel !== '..' && !rel.startsWith('../') && !isAbsolute(rel), 'stay inside the supplied bundle');
    const info = await stat(path);
    assert.ok(info.isFile() && info.size <= fileLimit, `${name} is a bounded served file`);
    headers(response, types[extname(name)] || 'application/octet-stream');
    response.setHeader('Content-Length', info.size);
    await pipeline(createReadStream(path), response);
  }
  const server = createServer((request, response) => {
    void (async () => {
      if (request.method !== 'GET') { response.writeHead(405); response.end(); return; }
      const path = decodeURIComponent(new URL(request.url, 'http://127.0.0.1').pathname);
      if (wrappers.has(path)) {
        headers(response, 'text/html; charset=utf-8'); response.end(wrappers.get(path)); return;
      }
      const mount = [...mounts.values()].find(value => path.startsWith(value.prefix));
      if (!mount) { response.writeHead(404); response.end('unknown fixture path'); return; }
      const name = path.slice(mount.prefix.length) || 'index.html';
      assert.ok(!name.split('/').some(part => part === '..' || part.includes('\\')), 'safe relative asset path');
      if (name !== font.url) { await file(response, name); return; }
      const record = {request: mount.requests.length + 1, url: new URL(request.url, origin).href,
        startedMs: performance.now(), status: null, fullySent: false, closedWithoutResponse: false};
      mount.requests.push(record);
      const kind = mount.plans[record.request - 1] || 'ok'; record.kind = kind;
      response.once('close', () => {
        record.closedMs = performance.now(); record.closedWithoutResponse = !response.writableEnded;
        mount.release(record.request);
      });
      if (kind === '404') {
        record.status = 404; headers(response, 'text/plain'); response.writeHead(404);
        response.end('font fixture refusal'); record.fullySent = true; return;
      }
      if (kind === 'timeout') {
        // Deliver genuine initial font bytes, then stall the body. Only the app's real 30s abort ends it.
        record.status = 200; headers(response, 'font/otf'); response.setHeader('Content-Length', font.bytes);
        response.writeHead(200);
        const stream = createReadStream(join(root, font.url), {start: 0, end: 65535});
        record.partialBytes = 0;
        for await (const chunk of stream) { response.write(chunk); record.partialBytes += chunk.length; }
        assert.equal(record.partialBytes, 65536, 'the stalled response contains genuine font bytes');
        await mount.wait(record.request);
        return;
      }
      if (kind === 'hold') await mount.wait(record.request);
      if (response.destroyed) return;
      record.status = 200;
      await file(response, name); record.fullySent = true;
    })().catch(error => {
      // A browser's abort during the deliberate timeout is observed through close, not hidden as success.
      failures.push(String(error));
      if (!response.destroyed) {
        if (!response.headersSent) response.writeHead(500);
        response.end('fixture server error');
      }
    });
  });
  // This fixture must not introduce a shorter server deadline than the production 30s abort.
  server.setTimeout(0);
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const address = server.address(); assert.ok(address && typeof address === 'object');
  origin = `http://127.0.0.1:${address.port}`;
  return {
    origin, failures,
    mount(name, plans = []) {
      assert.match(name, /^[a-z0-9-]+$/);
      const pending = new Map(); const released = new Set();
      const mount = {prefix: `/font-acceptance/${name}/`, plans, requests: [],
        wait(number) {
          if (released.has(number)) return Promise.resolve();
          assert.ok(!pending.has(number), 'one wait per held request');
          return new Promise(resolve => pending.set(number, resolve));
        },
        release(number = 1) { released.add(number); pending.get(number)?.(); pending.delete(number); },
      };
      assert.ok(!mounts.has(name)); mounts.set(name, mount);
      const url = new URL(mount.prefix, origin); url.search = 'store=memory&workers=0';
      return {...mount, url, release: mount.release};
    },
    iframe(name, mount) {
      assert.match(name, /^[a-z0-9-]+$/);
      const path = `/font-acceptance/${name}.html`;
      // Original fixture shell. Inset proves pointer routing goes through the actual embedded canvas.
      wrappers.set(path, `<!doctype html><meta charset="utf-8"><title>Font iframe fixture</title>
        <style>body{margin:0;background:#eee}iframe{position:absolute;left:40px;top:32px;width:960px;height:720px;border:0}</style>
        <iframe id="font_app" title="LightCraft font acceptance" src="${mount.url.href.replaceAll('&', '&amp;')}"></iframe>`);
      return new URL(path, origin);
    },
    async close() {
      for (const mount of mounts.values()) for (let number = 1; number <= mount.requests.length; number++) mount.release(number);
      const done = new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
      server.closeAllConnections(); await done;
    },
  };
}
