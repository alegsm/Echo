// @ts-nocheck
import { expect, test } from 'bun:test';
import { createReportedTranscriptDelivery } from '../src/lib/runtime/desktop/reported-transcript-delivery';

test('saved transcript reports the ordered clipboard outcome with ownership proof', async () => {
  const reports = [];
  const queue = createReportedTranscriptDelivery({
    client: { async reportDelivery(report) { reports.push(report); } },
    delivery: { async deliver() { return 'copied'; } }
  });
  await queue.enqueue(Promise.resolve(task(true)));
  expect(reports).toEqual([{
    insertionMode: 'copied',
    intentId: 'intent-a',
    sessionId: 'session-a',
    transcriptId: 'transcript-a'
  }]);
});

test('unsaved transcript is still delivered but skips the history acknowledgement', async () => {
  const reports = [];
  let delivered = 0;
  const queue = createReportedTranscriptDelivery({
    client: { async reportDelivery(report) { reports.push(report); } },
    delivery: { async deliver() { delivered += 1; return 'pasted'; } }
  });
  await queue.enqueue(Promise.resolve(task(false)));
  expect(delivered).toBe(1);
  expect(reports).toEqual([]);
});

test('delivery failure reports a typed failed insertion and keeps the queue reject visible', async () => {
  const reports = [];
  const queue = createReportedTranscriptDelivery({
    client: { async reportDelivery(report) { reports.push(report); } },
    delivery: {
      async deliver() {
        throw Object.assign(new Error('paste failed'), { code: 'insertion_failed' });
      }
    }
  });
  await expect(queue.enqueue(Promise.resolve(task(true)))).rejects.toThrow('paste failed');
  expect(reports[0]).toMatchObject({ errorCode: 'insertion_failed', insertionMode: 'failed' });
});

test('successful delivery acknowledgement failure is not reported a second time as delivery failure', async () => {
  const reports = [];
  const queue = createReportedTranscriptDelivery({
    client: {
      async reportDelivery(report) {
        reports.push(report);
        throw new Error('history acknowledgement failed');
      }
    },
    delivery: { async deliver() { return 'pasted'; } }
  });

  await expect(queue.enqueue(Promise.resolve(task(true)))).rejects.toThrow(
    'history acknowledgement failed'
  );
  expect(reports).toHaveLength(1);
  expect(reports[0].insertionMode).toBe('pasted');
});

function task(saved) {
  return {
    delivery: { pasteAfterTranscription: true, restoreClipboard: true },
    persistence: { saved },
    proof: { artifactId: 'artifact-a', intentId: 'intent-a', sessionId: 'session-a' },
    transcript: { id: 'transcript-a', text: 'hello' }
  };
}

// Keep: marker-only rows reserve their place without clipboard work or delivery reports.
test('marker-only failures skip delivery and reporting; disposal suppresses queued work', async () => {
  const delivered = [], reports = [];
  const queue = createReportedTranscriptDelivery({
    client: { async reportDelivery(report) { reports.push(report); } },
    delivery: { async deliver(transcript) { delivered.push(transcript.text); return 'copied'; } }
  });
  const marker = task(true);
  marker.transcript = { ...marker.transcript, text: '[Transcription gap]', errorCode: 'provider_failed', insertionMode: 'failed' };
  await queue.enqueue(Promise.resolve(marker));
  expect(delivered).toEqual([]);
  expect(reports).toEqual([]);
  await queue.enqueue(Promise.resolve(task(true)));
  expect(delivered).toEqual(['hello']);
  expect(reports).toHaveLength(1);
  const pending = queue.enqueue(Promise.resolve(task(true)));
  queue.dispose();
  await pending;
  expect(delivered).toEqual(['hello']);
});
