// Verifies generated assets; no app build or browser is launched.
import assert from 'node:assert/strict';
import {readFile, readdir, stat} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {join} from 'node:path';
export const font = Object.freeze({
  url: 'fonts/NotoSansCJKsc-Regular.otf', bytes: 16437364,
  sha256: '2c76254f6fc379fddfce0a7e84fb5385bb135d3e399294f6eeb6680d0365b74b',
});
export const fileLimit = 25 * 1024 * 1024;
export async function verifyFontAssets(directory) {
  const bytes = await readFile(join(directory, font.url));
  assert.equal(bytes.length, font.bytes);
  assert.equal(createHash('sha256').update(bytes).digest('hex'), font.sha256);
  assert.ok(bytes.length <= fileLimit);
  const descriptor = JSON.parse(await readFile(join(directory, 'fonts/chinese-ui.json'), 'utf8'));
  for (const [key, value] of Object.entries(font)) assert.equal(descriptor[key], value);
  const licence = await readFile(join(directory, descriptor.licence), 'utf8');
  const notice = await readFile(join(directory, descriptor.notice), 'utf8');
  assert.match(licence, /SIL OPEN FONT LICENSE Version 1.1/);
  assert.match(notice, /© 2014-2021 Adobe \(http:\/\/www\.adobe\.com\/\)\./);
  assert.match(notice, /Unmodified font/); assert.ok(notice.includes(font.sha256));
  return descriptor;
}
export async function verifyBundle(directory) {
  await verifyFontAssets(directory);
  async function walk(path) {
    for (const entry of await readdir(path, {withFileTypes: true})) {
      const file = join(path, entry.name);
      if (entry.isDirectory()) await walk(file);
      else assert.ok((await stat(file)).size <= fileLimit, `${file} exceeds 25 MiB`);
    }
  }
  await walk(directory);
  assert.ok((await stat(join(directory, 'lightcraft_web_bg.wasm'))).size > 0);
}
