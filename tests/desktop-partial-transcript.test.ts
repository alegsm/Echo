import { expect, test } from 'bun:test';
import { createPartialTranscriptState } from '../src/lib/runtime/desktop/partial-transcript-state';
import type { NativeCaptureBoundary } from '../src/lib/runtime/desktop/native-capture-client';

test('projection notifies on ordered segment eligibility, including text arriving first', async () => {
  const changes: unknown[] = [];
  const listeners = new Map<string, (event: { payload: unknown }) => void>();
  const state = createPartialTranscriptState((projection) => changes.push(projection));
  await state.start({
    invoke: async <T>() => undefined as T,
    async listen(name, listener) {
      listeners.set(name, listener as (event: { payload: unknown }) => void);
      return () => { listeners.delete(name); };
    }
  });
  const capture = (sequence: number, intentId: string, outcome: string) =>
    listeners.get('echo-audio-capture')?.({ payload: { sequence, intentId, outcome, phase: 'recording' } });
  state.begin('a');
  state.apply({ dictationId: 'a', ordinal: 0, text: ' first ' });
  expect(changes.at(-1)).toMatchObject({ dictationId: 'a', text: 'first', eligible: false, phase: 'active' });
  capture(2, 'foreign', 'segment_closed');
  capture(1, 'a', 'segment_closed');
  expect(changes).toHaveLength(2);
  capture(3, 'a', 'segment_closed');
  expect(changes.at(-1)).toMatchObject({ text: 'first', eligible: true });
  state.apply({ dictationId: 'a', ordinal: 2, text: ' second ' });
  expect(changes.at(-1)).toMatchObject({ text: 'first second', eligible: true });
  const accepted = changes.length;
  state.apply({ dictationId: 'a', ordinal: 2, text: 'duplicate' });
  state.apply({ dictationId: 'a', ordinal: 1, text: 'older' });
  state.apply({ dictationId: 'foreign', ordinal: 3, text: 'foreign' });
  expect(changes).toHaveLength(accepted);
  listeners.get('echo-stt-job')?.({ payload: { intentId: 'a', state: 'completed' } });
  expect(state.get()).toBeUndefined();
  expect(changes.at(-1)).toMatchObject({ dictationId: 'a', text: '', phase: 'completed' });
  state.clear('a');
  expect(changes.at(-1)).toMatchObject({ dictationId: 'a', phase: 'cleared' });
  state.apply({ dictationId: 'a', ordinal: 4, text: 'late' });
  expect(state.get()).toBeUndefined();
  state.dispose();
});

test('partial text keeps native ordinal order and rejects duplicate and stale events', () => {
  const state = createPartialTranscriptState();
  state.begin('a');
  state.apply({ dictationId: 'a', ordinal: 0, text: ' first ' });
  state.apply({ dictationId: 'a', ordinal: 2, text: ' second ' });
  expect(state.get()).toEqual({ dictationId: 'a', ordinal: 2, text: 'first second' });
  state.apply({ dictationId: 'a', ordinal: 2, text: 'duplicate' });
  state.apply({ dictationId: 'a', ordinal: 1, text: 'stale' });
  state.apply({ dictationId: 'old', ordinal: 3, text: 'foreign' });
  expect(state.get()?.text).toBe('first second');
  state.begin('b');
  state.clear('a');
  state.apply({ dictationId: 'a', ordinal: 3, text: 'late' });
  expect(state.get()).toEqual({ dictationId: 'b', ordinal: -1, text: '' });
  state.clear('b');
  state.apply({ dictationId: 'b', ordinal: 0, text: 'late' });
  expect(state.get()).toBeUndefined();
});

test('disposal clears partial text and prevents later events or captures from restoring it', () => {
  const state = createPartialTranscriptState();
  state.begin('a');
  state.apply({ dictationId: 'a', ordinal: 0, text: 'speech' });
  state.dispose();
  state.begin('b');
  state.apply({ dictationId: 'b', ordinal: 0, text: 'late' });
  expect(state.get()).toBeUndefined();
});

test('native terminal events clear matching text, including capture failures without identity', async () => {
  const listeners = new Map<string, (event: { payload: unknown }) => void>();
  const boundary: NativeCaptureBoundary = {
    invoke: async <T>() => undefined as T,
    async listen(name, listener) {
      listeners.set(name, listener as (event: { payload: unknown }) => void);
      return () => { listeners.delete(name); };
    }
  };
  const state = createPartialTranscriptState();
  await state.start(boundary);
  const send = (name: string, payload: unknown) => listeners.get(name)?.({ payload });
  state.begin('a');
  send('echo-audio-capture', { sequence: 1, intentId: 'a', phase: 'recording' });
  send('echo-partial-transcript', { dictationId: 'a', ordinal: 0, text: 'speech' });
  send('echo-audio-capture', { sequence: 2, intentId: null, phase: 'idle', error: 'device_unavailable' });
  expect(state.get()).toBeUndefined();
  state.begin('cancelled');
  send('echo-audio-capture', { sequence: 3, intentId: 'cancelled', phase: 'recording' });
  send('echo-partial-transcript', { dictationId: 'cancelled', ordinal: 0, text: 'speech' });
  send('echo-audio-capture', { sequence: 4, intentId: null, phase: 'idle', error: 'capture_cancelled' });
  expect(state.get()).toBeUndefined();
  state.begin('new');
  send('echo-audio-capture', { sequence: 1, intentId: 'new', phase: 'idle', error: 'device_unavailable' });
  expect(state.get()?.dictationId).toBe('new');
  for (const terminal of ['failed', 'canceled', 'completed']) {
    state.begin('a');
    send('echo-partial-transcript', { dictationId: 'a', ordinal: 0, text: 'speech' });
    send('echo-stt-job', { intentId: 'old', state: terminal });
    expect(state.get()?.text).toBe('speech');
    send('echo-stt-job', { intentId: 'a', state: terminal });
    expect(state.get()).toBeUndefined();
  }
  state.dispose();
  expect(listeners.size).toBe(0);
});

test('a listener that resolves after disposal is immediately removed', async () => {
  let resolveListen: (unlisten: () => void) => void = () => undefined;
  let removed = false;
  const boundary: NativeCaptureBoundary = {
    invoke: async <T>() => undefined as T,
    listen: () => new Promise((resolve) => { resolveListen = resolve; })
  };
  const state = createPartialTranscriptState();
  const starting = state.start(boundary);
  state.dispose();
  resolveListen(() => { removed = true; });
  await starting;
  expect(removed).toBe(true);
});

// Keep: recording telemetry errors must not retire the live preview.
test('nonterminal PCM and underrun errors preserve preview and accept later partials', async () => {
  const listeners = new Map<string, (event: { payload: unknown }) => void>();
  const state = createPartialTranscriptState();
  await state.start({ invoke: async <T>() => undefined as T, async listen(name, listener) {
    listeners.set(name, listener as (event: { payload: unknown }) => void); return () => undefined;
  } });
  state.begin('a');
  state.apply({ dictationId: 'a', ordinal: 0, text: 'first' });
  for (const [index, error] of ['pcm_limit', 'buffer_underrun'].entries()) {
    listeners.get('echo-audio-capture')?.({ payload: { sequence: index + 1, intentId: 'a', phase: 'recording', error } });
    state.apply({ dictationId: 'a', ordinal: index + 1, text: 'next' });
    expect(state.get()?.text).toBe(['first', ...Array(index + 1).fill('next')].join(' '));
  }
  state.dispose();
});

// Keep: status for a later ordinal must arrive before ordered text can advance.
test('new notices bypass text ordering without latching status onto later partials', () => {
  const state = createPartialTranscriptState();
  state.begin('a');
  state.apply({ dictationId: 'a', ordinal: 0, text: 'speech' });
  for (const status of ['retrying', 'gap', 'retrying'] as const) {
    state.apply({ dictationId: 'a', ordinal: 7, text: '', status });
    expect(state.getProjection()).toMatchObject({ text: 'speech', ordinal: 0, status });
  }
  state.apply({ dictationId: 'a', ordinal: 1, text: 'later' });
  expect(state.get()?.text).toBe('speech later');
  expect(state.getProjection()?.status).toBeUndefined();
});
