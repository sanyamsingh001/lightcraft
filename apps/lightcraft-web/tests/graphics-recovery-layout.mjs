// Static Chromium regression: production HTML, synthetic context loss, no WASM or archives.
// LIGHTCRAFT_PLAYWRIGHT_MODULE can point to an existing Playwright installation.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
const {chromium} = await import(process.env.LIGHTCRAFT_PLAYWRIGHT_MODULE || 'playwright');
const html = readFileSync(new URL('../index.html', import.meta.url), 'utf8')
  .replace(/<script type="module">[\s\S]*?<\/script>/, '');
const browser = await chromium.launch({headless: true});
try {
  const page = await browser.newPage();
  page.setDefaultTimeout(30000);
  for (const [width, height] of [[1280,720],[320,480],[320,320],[320,200],[800,200],[240,160]]) {
    await page.setViewportSize({width, height});
    await page.setContent(html);
    await page.evaluate(() => {
      globalThis.recoveryCalls = [];
      globalThis.lightcraft = {command: async (id, params) => {
        recoveryCalls.push(id);
        if (id === 'library.state') return '{"selection":{"ids":[4,7],"active":7}}';
        if (id === 'app.export' && JSON.parse(params).ids?.join(',') !== '7') throw new Error('export must target the active photo');
        return '{}';
      }};
      globalThis.confirm = () => { recoveryCalls.push('reload'); return false; };
      document.getElementById('lightcraft_canvas').dispatchEvent(new Event('webglcontextlost', {cancelable: true}));
    });
    for (const longStatus of [false, true]) {
      for (const id of ['title', 'export', 'backup', 'reload']) {
        await page.evaluate(long => {
          document.getElementById('lightcraft_recovery_status').textContent = long ? 'Backup failed: ' + 'long-storage-path/'.repeat(30) : '';
        }, longStatus);
        const element = page.locator(`#lightcraft_recovery_${id}`);
        await element.scrollIntoViewIfNeeded();
        const rect = await element.boundingBox();
        assert.ok(rect && rect.x >= 0 && rect.y >= 0 && rect.x + rect.width <= width + 1 && rect.y + rect.height <= height + 1,
          `${width}x${height}, long status ${longStatus}: ${id} must fit after scrolling (${JSON.stringify(rect)})`);
        if (id !== 'title') await element.click();
      }
      assert.equal(await page.evaluate(() => {
        const panel = document.getElementById('lightcraft_recovery');
        return panel.scrollWidth <= panel.clientWidth;
      }), true, `${width}x${height}: long status must not require horizontal scrolling`);
    }
    assert.deepEqual(await page.evaluate(() => recoveryCalls), ['library.state', 'app.export', 'file.backupLibrary', 'reload', 'library.state', 'app.export', 'file.backupLibrary', 'reload']);
    console.log(`PASS ${width}x${height}: heading and every action reachable, including long status`);
  }
} finally { await browser.close(); }
