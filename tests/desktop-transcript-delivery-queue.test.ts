// @ts-nocheck
import { expect, test } from 'bun:test';
import { createTranscriptDeliveryQueue } from '../src/lib/runtime/desktop/transcript-delivery-queue';

test('delivery follows recording order and an earlier failure advances the queue', async () => {
  const delivered = [];
  const queue = createTranscriptDeliveryQueue({
    async deliver(transcript) { delivered.push(transcript.id); }
  });
  const first = deferred();
  const second = deferred();
  const third = deferred();
  void queue.enqueue(first.promise).catch(() => undefined);
  void queue.enqueue(second.promise).catch(() => undefined);
  void queue.enqueue(third.promise).catch(() => undefined);
  second.resolve({ id: 'second' });
  third.resolve({ id: 'third' });
  await Promise.resolve();
  expect(delivered).toEqual([]);
  first.reject(new Error('provider failed'));
  await waitFor(() => delivered.length === 2);
  expect(delivered).toEqual(['second', 'third']);
});

test('dispose suppresses pending clipboard delivery', async () => {
  const delivered = [];
  const queue = createTranscriptDeliveryQueue({
    async deliver(transcript) { delivered.push(transcript.id); }
  });
  const pending = deferred();
  void queue.enqueue(pending.promise);
  queue.dispose();
  pending.resolve({ id: 'late' });
  await Promise.resolve();
  await Promise.resolve();
  expect(delivered).toEqual([]);
});

test('later rejection is consumed while an earlier transcription is pending', async () => {
  const first = deferred();
  const queue = createTranscriptDeliveryQueue({ async deliver() {} });
  const firstDelivery = queue.enqueue(first.promise);
  const secondDelivery = queue.enqueue(Promise.reject(new Error('later failed')));
  await Promise.resolve();
  first.resolve({ id: 'first' });

  await firstDelivery;
  await expect(secondDelivery).rejects.toThrow('later failed');
});

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((resolveValue, rejectValue) => { resolve = resolveValue; reject = rejectValue; });
  return { promise, reject, resolve };
}

async function waitFor(predicate) {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    if (predicate()) return;
    await Promise.resolve();
  }
  throw new Error('Condition was not met');
}
