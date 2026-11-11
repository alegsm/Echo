// @ts-nocheck
import { expect, test } from 'bun:test';
import { createDiagnosticEventWriter } from '../src/lib/runtime/diagnostic-event-writer';

test('enabled diagnostics send structured requests without renderer provenance', async () => {
  const harness = createHarness();
  harness.enabled = true;
  const event = { area: 'recording', event: 'clipboard_copy_failed', level: 'warn', fields: { code: 'clipboard_failed' } };
  expect(await harness.writer.write(event)).toBe(true);
  expect(harness.requests).toEqual([event]);
});

test('an event called while off stays suppressed after an immediate enable', async () => {
  const harness = createHarness();
  const pending = harness.writer.write({ area: 'diagnostics', event: 'enabled' });
  harness.enabled = true;
  expect(await pending).toBe(false);
  expect(harness.requests).toEqual([]);
  expect(harness.statuses.at(-1)?.state).toBe('off');
});

test('sink failures use fixed status without exposing native error text or names', async () => {
  const harness = createHarness({ sink: async () => { throw { name: 'privateName', message: 'privateText' }; } });
  harness.enabled = true;
  expect(await harness.writer.write({ area: 'diagnostics', event: 'enabled' })).toBe(false);
  expect(harness.statuses.at(-1)).toEqual({ state: 'error', message: 'Could not write diagnostic log.' });
});

test('queued events and late sink failures stay off after disable', async () => {
  let rejectSink;
  let calls = 0;
  const harness = createHarness({ sink: () => {
    calls += 1;
    return new Promise((_resolve, reject) => { rejectSink = reject; });
  } });
  harness.enabled = true;
  const first = harness.writer.write({ area: 'diagnostics', event: 'enabled' });
  const queued = harness.writer.write({ area: 'diagnostics', event: 'enabled' });
  await waitFor(() => rejectSink !== undefined);
  harness.enabled = false;
  rejectSink(new Error('private late failure'));
  expect(await first).toBe(false);
  expect(await queued).toBe(false);
  expect(calls).toBe(1);
  expect(harness.statuses.at(-1)?.state).toBe('off');
});

test('native suppression is returned as off and cannot report acceptance', async () => {
  const harness = createHarness({ sink: async () => false });
  harness.enabled = true;
  expect(await harness.writer.write({ area: 'diagnostics', event: 'enabled' })).toBe(false);
  expect(harness.statuses.at(-1)?.state).toBe('off');
});

function createHarness(overrides = {}) {
  const harness = { enabled: false, requests: [], statuses: [], writer: undefined };
  harness.writer = createDiagnosticEventWriter({
    isEnabled: () => harness.enabled,
    sink: overrides.sink ?? (async (request) => { harness.requests.push(request); return true; }),
    onStatus: (status) => harness.statuses.push(status)
  });
  return harness;
}

async function waitFor(predicate) {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    if (predicate()) return;
    await Promise.resolve();
  }
  throw new Error('Condition was not met');
}
