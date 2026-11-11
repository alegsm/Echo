// @ts-nocheck
import { expect, test } from 'bun:test';
import { createNativeHistoryStore } from '../src/lib/runtime/desktop/native-history-store';

test('native history events invalidate revision cache and stop after disposal', async () => {
  let listener;
  let unlistenCalls = 0;
  const events = [];
  const client = {
    async listenHistory(next) { listener = next; return () => { unlistenCalls += 1; }; },
    async listHistory() { return { revision: 1, totalCount: 1, records: [{ revision: 1, record: transcript('a') }] }; }
  };
  const store = createNativeHistoryStore({ client, onChanged: (event) => events.push(event.action) });
  await store.start();
  await store.list(10);
  listener({ action: 'append', historyRevision: 2, recordId: 'b' });
  store.dispose();
  listener({ action: 'clear', historyRevision: 3 });

  expect(events).toEqual(['append']);
  expect(unlistenCalls).toBe(1);
});

test('large history requests follow bounded native cursors until every row is reachable', async () => {
  const records = Array.from({ length: 508 }, (_, index) => ({
    record: transcript(String(index)),
    revision: 1
  }));
  const calls = [];
  const client = {
    async listHistory(limit, cursor) {
      calls.push([limit, cursor]);
      if (limit > 200) throw new Error('history.limit');
      const start = cursor ? Number(cursor.id) + 1 : 0;
      const pageRecords = records.slice(start, start + limit);
      const final = pageRecords.at(-1);
      return {
        nextCursor: start + pageRecords.length < records.length && final
          ? { createdAt: final.record.createdAt, id: final.record.id }
          : undefined,
        records: pageRecords,
        revision: 4,
        totalCount: records.length
      };
    }
  };
  const store = createNativeHistoryStore({ client, onChanged() {} });

  const loaded = await store.list(508);

  expect(loaded).toHaveLength(508);
  expect(loaded.at(-1)?.id).toBe('507');
  expect(store.getTotalCount()).toBe(508);
  expect(calls.map(([limit]) => limit)).toEqual([200, 200, 108]);
});

function transcript(id) {
  return {
    id,
    text: id,
    createdAt: '2026-09-09T00:00:00.000Z',
    durationMs: 200,
    providerId: 'groq-whisper-turbo',
    timings: { recordingMs: 200, blobReadyMs: 0, uploadMs: 0, stopToTranscriptMs: 20 },
    audio: { mimeType: 'audio/wav', byteSize: 100 },
    insertionMode: 'manual',
    source: 'desktop'
  };
}
