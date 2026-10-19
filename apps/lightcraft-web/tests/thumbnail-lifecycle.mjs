// Optional combined-feature extension. The WEB-alone harness never imports this module.
// Its caller detects the command in engine.commands; command/progress failures propagate.
import assert from 'node:assert/strict';

export async function startThumbnailPlan({page, command, until}) {
  await command(page, 'library.buildThumbnails', {wait: false});
  return {
    async held() {
      const pending = await command(page, 'library.previewProgress');
      assert.equal(pending.failed, 0, 'an asynchronous original miss is not a terminal render error');
      assert.equal(pending.running, true, 'thumbnail work must wait for the healthy original');
    },
    async complete(terminal) {
      let result;
      await until(async () => {
        result = await command(page, 'library.previewProgress');
        return result && !result.running;
      });
      assert.ok(result.total > 0);
      assert.equal(result.what, 'thumbnails');
      if (terminal) assert.ok(result.failed > 0, 'a terminal read error must finish with failures');
      else {
        assert.equal(result.failed, 0);
        assert.equal(result.error, null);
        assert.equal(result.done, result.total);
        assert.ok(result.skipped <= result.done);
      }
    },
  };
}
