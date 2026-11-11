// @ts-nocheck
import { expect, test } from 'bun:test';
import { createDesktopTranscriptDelivery } from '../src/lib/runtime/desktop/transcript-delivery';

// Keep: the renderer forwards frozen policy in one native invocation and exposes only its outcome.
for (const [pasteAfterTranscription, restoreClipboard, outcome] of [
  [true, true, 'pasted'], [true, false, 'pasted'], [false, true, 'copied']
]) {
  test(`native delivery forwards frozen policy ${pasteAfterTranscription}/${restoreClipboard}`, async () => {
    const calls = [];
    const delivery = createDesktopTranscriptDelivery({ boundary: {
      async invoke(command, args) { calls.push([command, args]); return outcome; }
    } });
    expect(await delivery.deliver({ id: 'a', text: 'hello' }, {
      pasteAfterTranscription, restoreClipboard
    })).toBe(outcome);
    expect(calls).toEqual([['deliver_transcript', {
      request: { text: 'hello', pasteAfterTranscription, restoreClipboard }
    }]]);
  });
}

// Keep: native failures retain their public classification and never expose arbitrary native details.
for (const code of ['clipboard_failed', 'insertion_failed']) {
  test(`native ${code} remains typed`, async () => {
    const delivery = createDesktopTranscriptDelivery({ boundary: {
      async invoke() { throw { code, message: 'private native detail' }; }
    } });
    await expect(delivery.deliver({ text: 'hello' }, {
      pasteAfterTranscription: true, restoreClipboard: true
    })).rejects.toMatchObject({ code, message: code === 'clipboard_failed'
      ? 'Transcript could not be copied.' : 'Transcript could not be pasted.' });
  });
}

// Keep: explicit Copy invokes only the ordinary native copy command, with no delivery or report.
test('explicit Copy preserves its independent command and feedback failure', async () => {
  const calls = [];
  let fail = false;
  const delivery = createDesktopTranscriptDelivery({ boundary: {
    async invoke(command, args) {
      calls.push([command, args]);
      if (fail) throw new Error('private clipboard detail');
    }
  } });
  await delivery.copy({ id: 'a', text: 'hello' });
  expect(calls).toEqual([['copy_transcript', { request: { text: 'hello' } }]]);
  fail = true;
  await expect(delivery.copy({ text: 'hello' })).rejects.toMatchObject({
    code: 'clipboard_failed', message: 'Transcript could not be copied.'
  });
});
