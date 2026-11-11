// @ts-nocheck
import { expect, test } from 'bun:test';
import { createCaptureIntentController } from '../src/lib/runtime/desktop/capture-intent-controller';
import { createNativeCaptureClient } from '../src/lib/runtime/desktop/native-capture-client';
import { createNativeTranscriptionClient } from '../src/lib/runtime/desktop/native-transcription-client';
import { createDesktopRuntime } from '../src/lib/runtime/desktop/desktop-runtime';
import { defaultSettings } from '../src/lib/core/settings';
import { createNativeStorageClient } from '../src/lib/runtime/desktop/native-storage-client';
import { createNativeRuntimeState } from '../src/lib/runtime/desktop/native-runtime-state';

test('native client sends metadata-only capture commands', async () => {
  const boundary = createBoundary();
  const client = createNativeCaptureClient(boundary);
  boundary.responses.set('start_audio_capture', session('intent-a', 'session-a'));
  boundary.responses.set('stop_audio_capture', artifact('session-a'));

  await client.start('push_to_talk', 'intent-a');
  await client.stop('session-a', 'intent-a');
  await client.cancel('intent-a', 'session-a', 'test');

  expect(boundary.calls).toEqual([
    ['start_audio_capture', { request: { purpose: 'push_to_talk', intentId: 'intent-a' } }],
    ['stop_audio_capture', { request: { sessionId: 'session-a', intentId: 'intent-a' } }],
    ['cancel_audio_capture', { request: { intentId: 'intent-a', sessionId: 'session-a', reason: 'test' } }]
  ]);
  expect(JSON.stringify(boundary.calls)).not.toContain('audioBase64');
});

test('transcription sends artifact ownership proof without renderer settings or audio', async () => {
  const boundary = createBoundary();
  const client = createNativeTranscriptionClient(boundary);
  const proof = { artifactId: 'artifact-a', intentId: 'intent-a', sessionId: 'session-a' };
  boundary.responses.set('transcribe_audio_artifact', { id: 'transcript-a', text: 'hello' });

  await client.transcribe(proof);
  await client.cancel('session-a', 'intent-a');
  await client.reportDelivery({
    intentId: 'intent-a', insertionMode: 'copied', sessionId: 'session-a', transcriptId: 'transcript-a'
  });
  expect(boundary.calls).toEqual([
    ['transcribe_audio_artifact', { request: proof }],
    ['cancel_transcription_job', { request: { sessionId: 'session-a', intentId: 'intent-a' } }],
    ['report_transcript_delivery', {
      request: {
        intentId: 'intent-a', insertionMode: 'copied', sessionId: 'session-a', transcriptId: 'transcript-a'
      }
    }]
  ]);
  expect(JSON.stringify(boundary.calls)).not.toMatch(/audioBase64|apiKey|providerId|prompt|language/);
});

test('desktop runtime probes providers with only a provider ID', async () => {
  const boundary = createBoundary();
  const response = {
    ok: true,
    providerId: 'openai-gpt-4o-mini-transcribe',
    providerModel: 'gpt-4o-mini-transcribe',
    timings: { backendMs: 20, providerMs: 18 }
  };
  boundary.responses.set('probe_provider', response);
  const runtime = createRuntime(boundary);
  try {
    expect(await runtime.probeProviderKey({ providerId: response.providerId })).toEqual(response);
    expect(boundary.calls.filter(([command]) => command === 'probe_provider')).toEqual([
      ['probe_provider', { request: { providerId: response.providerId } }]
    ]);
  } finally {
    await runtime.dispose();
  }
});

test('storage client uses revision-checked writes and reload-only event payloads', async () => {
  const boundary = createBoundary();
  const client = createNativeStorageClient(boundary);
  boundary.responses.set('get_persisted_settings', { revision: 4, settings: {} });
  boundary.responses.set('update_persisted_settings', {
    changedFields: ['language'],
    current: { revision: 5, settings: {} },
    previous: { revision: 4, settings: {} }
  });
  await client.getSettings();
  await client.updateSettings(4, { language: 'en' });
  await client.updateSettings(5, { fallbackShortcut: undefined, historyRetentionDays: undefined });

  expect(boundary.calls).toEqual([
    ['get_persisted_settings', undefined],
    ['update_persisted_settings', { expectedRevision: 4, patch: { language: 'en' } }],
    ['update_persisted_settings', {
      expectedRevision: 5,
      patch: { fallbackShortcut: null, historyRetentionDays: null }
    }]
  ]);
  expect(JSON.parse(JSON.stringify(boundary.calls.at(-1)?.[1]))).toEqual({
    expectedRevision: 5,
    patch: { fallbackShortcut: null, historyRetentionDays: null }
  });
});

test('desktop state applies native envelopes monotonically and stops after disposal', async () => {
  const boundary = createBoundary();
  const snapshotResult = deferred();
  boundary.responses.set('get_desktop_runtime_snapshot', snapshotResult.promise);
  const state = createNativeRuntimeState(boundary);
  const seen = [];
  state.subscribe((snapshot) => seen.push(snapshot.state));
  const starting = state.start();
  await waitFor(() => boundary.listener !== undefined);
  boundary.listener({ payload: { sequence: 4, snapshot: echoSnapshot('recording') } });
  snapshotResult.resolve({ sequence: 3, snapshot: echoSnapshot('requesting_microphone') });
  await starting;
  expect(seen).toEqual(['recording']);
  state.apply({ sequence: 5, snapshot: echoSnapshot('done') });
  expect(state.getSnapshot()?.state).toBe('done');
  state.dispose();
  state.apply({ sequence: 6, snapshot: echoSnapshot('error') });
  expect(state.getSnapshot()?.state).toBe('done');
});

test('release during native start cancels by intent and cannot resurrect recording', async () => {
  const startResult = deferred();
  const calls = [];
  const client = createClient({
    start: (...args) => { calls.push(['start', ...args]); return startResult.promise; },
    cancel: (...args) => { calls.push(['cancel', ...args]); return Promise.resolve(); }
  });
  const controller = createCaptureIntentController({
    client,
    createIntentId: () => 'intent-race',
    purpose: 'push_to_talk'
  });

  const starting = controller.press();
  await Promise.resolve();
  const stopping = controller.release();
  startResult.reject(captureError('capture_cancelled'));

  expect(await starting).toEqual({ status: 'cancelled', intentId: 'intent-race' });
  expect(await stopping).toEqual({ status: 'cancelled', intentId: 'intent-race' });
  expect(calls).toContainEqual(['cancel', 'intent-race', undefined, 'released_before_start']);
  expect(controller.isHeld()).toBe(false);
});

test('active recording returns artifact metadata and serializes the next press behind stop', async () => {
  const stopResult = deferred();
  const calls = [];
  const client = createClient({
    start: async (_purpose, intentId) => {
      calls.push(['start', intentId]);
      return session(intentId, `session-${intentId}`);
    },
    stop: async (sessionId, intentId) => {
      calls.push(['stop', intentId]);
      if (intentId === 'intent-1') return stopResult.promise;
      return artifact(sessionId);
    }
  });
  let intent = 0;
  const controller = createCaptureIntentController({
    client,
    createIntentId: () => `intent-${++intent}`,
    purpose: 'push_to_talk'
  });

  await controller.press();
  const firstStop = controller.release();
  const secondStart = controller.press();
  await Promise.resolve();
  expect(calls).not.toContainEqual(['start', 'intent-2']);
  stopResult.resolve(artifact('session-intent-1'));
  expect((await firstStop).status).toBe('captured');
  expect((await secondStart).status).toBe('recording');
});

test('dispose cancels ownership and suppresses a late start response', async () => {
  const startResult = deferred();
  const calls = [];
  const client = createClient({
    start: () => startResult.promise,
    cancel: (...args) => { calls.push(args); return Promise.resolve(); }
  });
  const controller = createCaptureIntentController({
    client,
    createIntentId: () => 'intent-dispose',
    purpose: 'push_to_talk'
  });
  const starting = controller.press();
  await Promise.resolve();
  const disposing = controller.dispose();
  startResult.resolve(session('intent-dispose', 'session-late'));

  expect((await starting).status).toBe('cancelled');
  await disposing;
  expect(calls[0]).toEqual(['intent-dispose', undefined, 'renderer_disposed']);
  expect(controller.isHeld()).toBe(false);
});

function createRuntime(boundary, overrides = {}) {
  boundary.responses.set('get_persisted_settings', { revision: 1, settings: defaultSettings });
  boundary.responses.set('get_desktop_runtime_snapshot', { sequence: 1, snapshot: echoSnapshot('idle') });
  return createDesktopRuntime({
    boundary: { ...boundary, invoke: boundary.invoke.bind(boundary), listen: boundary.listen.bind(boundary) },
    hotkeyBoundary: { setFnCaptureEnabled: async () => undefined },
    hotkeyOwner: false,
    syncDiagnostics: async () => undefined,
    ...overrides
  });
}

// Keep: native projections must finish loading before capture can start.
test('capture waits for native settings and snapshot initialization', async () => {
  const boundary = createBoundary();
  const runtime = createRuntime(boundary);
  const settings = deferred();
  const snapshot = deferred();
  boundary.responses.set('get_persisted_settings', settings.promise);
  boundary.responses.set('get_desktop_runtime_snapshot', snapshot.promise);
  boundary.responses.set('start_audio_capture', session('intent', 'session'));
  const starting = runtime.startRecording();
  try {
    await flush();
    expect(boundary.calls.some(([name]) => name === 'start_audio_capture')).toBe(false);
    settings.resolve({ revision: 0, settings: defaultSettings });
    await flush();
    expect(boundary.calls.some(([name]) => name === 'start_audio_capture')).toBe(false);
    snapshot.resolve({ sequence: 1, snapshot: echoSnapshot('idle') });
    await starting;
    expect(boundary.calls.some(([name]) => name === 'start_audio_capture')).toBe(true);
  } finally {
    settings.resolve({ revision: 0, settings: defaultSettings });
    snapshot.resolve({ sequence: 1, snapshot: echoSnapshot('idle') });
    await starting.catch(() => undefined);
    await runtime.dispose();
  }
});

// Keep: disposal during native initialization cannot attach late listeners or hotkeys.
test('disposal during native settings load removes subscriptions and stops startup', async () => {
  const boundary = createBoundary();
  const runtime = createRuntime(boundary, { hotkeyOwner: true });
  const settings = deferred();
  boundary.responses.set('get_persisted_settings', settings.promise);
  const pending = runtime.getSnapshot();
  await flush();
  await runtime.dispose();
  settings.resolve({ revision: 0, settings: defaultSettings });
  await pending;
  expect(boundary.unlistenCalls).toBe(3);
  expect(boundary.listenedEvents).not.toContain('echo-credentials-updated');
  expect(boundary.listenedEvents).not.toContain('echo-partial-transcript');
});

test('runtime retains a tap during startup, emits its mode and replays it to a late subscriber', async () => {
  const starting = deferred();
  const h = await createHotkeyRuntime({ starting });
  h.press();
  await waitFor(() => h.commands('start_audio_capture').length === 1);
  h.clock.advance(100);
  h.release();
  h.clock.advance(80);
  expect(h.modes).toEqual([false, true]);
  expect(h.commands('cancel_audio_capture')).toHaveLength(0);
  expect(h.commands('stop_audio_capture')).toHaveLength(0);
  starting.resolve();
  await waitFor(() => h.snapshot().state === 'recording');
  const replay = [];
  const unsubscribe = h.runtime.subscribe((event) => {
    if (event.type === 'hands_free_changed') replay.push(event.active);
  });
  expect(replay).toEqual([true]);
  h.press();
  h.clock.advance(600);
  h.release();
  h.clock.advance(80);
  await waitFor(() => h.commands('report_transcript_delivery').length === 1);
  expect(h.commands('change_fn_key_preference')).toHaveLength(0);
  expect(h.commands('start_audio_capture')).toHaveLength(1);
  expect(h.commands('stop_audio_capture')).toHaveLength(1);
  expect(h.delivered).toEqual(['hello']);
  expect(h.pastes()).toBe(1);
  expect(h.modes).toEqual([false, true, false]);
  unsubscribe();
  await h.runtime.dispose();
});

test('failed startup resets a latched tap and permits a fresh native capture', async () => {
  const starting = deferred();
  const h = await createHotkeyRuntime({ starting });
  h.press();
  await waitFor(() => h.commands('start_audio_capture').length === 1);
  h.release();
  h.clock.advance(80);
  starting.reject(captureError('microphone_unavailable'));
  await waitFor(() => h.modes.at(-1) === false && h.modes.length > 1);
  await flush();
  h.press();
  await waitFor(() => h.commands('start_audio_capture').length === 2);
  await waitFor(() => h.snapshot().state === 'recording');
  expect(h.commands('start_audio_capture')[0][1].request.intentId)
    .not.toBe(h.commands('start_audio_capture')[1][1].request.intentId);
  expect(h.commands('stop_audio_capture')).toHaveLength(0);
  await h.runtime.dispose();
});

test('a current native exit during startup resets after the start command settles', async () => {
  const started = deferred();
  const h = await createHotkeyRuntime({ started });
  h.press();
  await waitFor(() => h.snapshot().state === 'recording');
  h.release();
  h.clock.advance(80);
  h.publish('error');
  await flush();
  expect(h.commands('cancel_audio_capture')).toHaveLength(0);
  expect(h.modes).toEqual([false, true]);
  started.resolve();
  await waitFor(() => h.commands('cancel_audio_capture').length === 1);
  await flush();
  expect(h.modes).toEqual([false, true, false]);
  expect(h.commands('stop_audio_capture')).toHaveLength(0);
  await h.runtime.dispose();
});

for (const exit of ['idle', 'error']) {
  test(`an unexpected recording exit to ${exit} clears retained capture intent`, async () => {
    const h = await createHotkeyRuntime();
    await h.latch();
    const firstIntent = h.commands('start_audio_capture')[0][1].request.intentId;
    h.publish(exit);
    h.emitCapture(h.captureEvent({ phase: 'idle', outcome: 'stream_error', error: 'stream_backend_error' }));
    await waitFor(() => h.commands('cancel_audio_capture').length === 1);
    expect(h.commands('cancel_audio_capture')[0][1].request.intentId).toBe(firstIntent);
    expect(h.modes).toEqual([false, true, false]);
    expect(h.commands('stop_audio_capture')).toHaveLength(0);
    h.press();
    await waitFor(() => h.commands('start_audio_capture').length === 2);
    expect(h.commands('start_audio_capture')[1][1].request.intentId).not.toBe(firstIntent);
    await h.runtime.dispose();
  });
}

test('cancelRecording resets hands-free without a send and stale release timers leave the next capture alone', async () => {
  const h = await createHotkeyRuntime();
  await h.latch();
  h.press();
  h.release();
  const staleTimer = h.clock.callbacks.at(-1);
  await h.runtime.cancelRecording();
  expect(h.modes).toEqual([false, true, false]);
  expect(h.commands('cancel_audio_capture')).toHaveLength(1);
  h.press();
  await waitFor(() => h.commands('start_audio_capture').length === 2);
  staleTimer();
  h.clock.advance(1000);
  await flush();
  expect(h.commands('stop_audio_capture')).toHaveLength(0);
  expect(h.commands('transcribe_audio_artifact')).toHaveLength(0);
  expect(h.commands('cancel_audio_capture')).toHaveLength(1);
  expect(h.snapshot().state).toBe('recording');
  await h.runtime.dispose();
});

test('explicit stop resets hands-free and follows the existing transcription and delivery path', async () => {
  const h = await createHotkeyRuntime();
  await h.latch();
  await h.runtime.stopRecording();
  expect(h.modes).toEqual([false, true, false]);
  expect(h.commands('stop_audio_capture')).toHaveLength(1);
  expect(h.commands('cancel_audio_capture')).toHaveLength(0);
  expect(h.delivered).toEqual(['hello']);
  expect(h.pastes()).toBe(1);
  await h.runtime.dispose();
});

test('settings retain an identical shortcut and cancel hands-free when the shortcut changes', async () => {
  const h = await createHotkeyRuntime();
  await h.latch();
  await h.runtime.updateSettings({ language: 'en' });
  await flush();
  expect(h.modes).toEqual([false, true]);
  expect(h.commands('cancel_audio_capture')).toHaveLength(0);
  await h.runtime.updateSettings({ pushToTalkShortcut: 'Replacement' });
  await waitFor(() => h.shortcuts.has('Replacement'));
  expect(h.commands('cancel_audio_capture')).toHaveLength(1);
  expect(h.modes).toEqual([false, true, false]);
  h.shortcuts.get('Replacement')({ state: 'Pressed' });
  await waitFor(() => h.commands('start_audio_capture').length === 2);
  await h.runtime.dispose();
});

test('disposal resets hands-free, cancels capture and prevents a late release from sending', async () => {
  const h = await createHotkeyRuntime();
  await h.latch();
  h.press();
  h.release();
  const staleTimer = h.clock.callbacks.at(-1);
  await h.runtime.dispose();
  staleTimer();
  h.clock.advance(1000);
  await flush();
  expect(h.modes).toEqual([false, true, false]);
  expect(h.commands('cancel_audio_capture')).toHaveLength(1);
  expect(h.commands('stop_audio_capture')).toHaveLength(0);
  expect(h.delivered).toEqual([]);
});

test('resetting a newer capture leaves an older transcription and its delivery intact', async () => {
  const transcription = deferred();
  const h = await createHotkeyRuntime({ transcription });
  h.press();
  await waitFor(() => h.snapshot().state === 'recording');
  h.clock.advance(301);
  h.release();
  h.clock.advance(80);
  await waitFor(() => h.commands('transcribe_audio_artifact').length === 1);
  await h.latch();
  h.publish('error');
  h.emitCapture(h.captureEvent({ phase: 'idle', outcome: 'stream_error', error: 'stream_backend_error' }));
  await waitFor(() => h.commands('cancel_audio_capture').length === 1);
  transcription.resolve();
  await waitFor(() => h.commands('report_transcript_delivery').length === 1);
  expect(h.commands('cancel_transcription_job')).toHaveLength(0);
  expect(h.delivered).toEqual(['hello']);
  expect(h.modes.at(-1)).toBe(false);
  await h.runtime.dispose();
});

test('the previous native stop cannot reset a newer press waiting to acquire capture', async () => {
  const stopping = deferred();
  const h = await createHotkeyRuntime({ stopping });
  h.press();
  await waitFor(() => h.snapshot().state === 'recording');
  h.clock.advance(301);
  h.release();
  h.clock.advance(80);
  await waitFor(() => h.commands('stop_audio_capture').length === 1);
  h.press();
  await flush();
  expect(h.commands('start_audio_capture')).toHaveLength(1);
  stopping.resolve();
  await waitFor(() => h.commands('start_audio_capture').length === 2);
  await waitFor(() => h.snapshot().state === 'recording');
  expect(h.commands('cancel_audio_capture')).toHaveLength(0);
  h.release();
  h.clock.advance(80);
  expect(h.modes.at(-1)).toBe(true);
  await h.runtime.dispose();
});

test('a previous exit delivered after stop returns cannot cancel the newer native start', async () => {
  const transcription = deferred();
  const h = await createHotkeyRuntime({ lateStop: true, transcription });
  h.press();
  await waitFor(() => h.snapshot().state === 'recording');
  h.clock.advance(301);
  h.release();
  h.clock.advance(80);
  await waitFor(() => h.commands('transcribe_audio_artifact').length === 1);
  h.press();
  await waitFor(() => h.commands('start_audio_capture').length === 2);
  await flush();
  expect(h.commands('cancel_audio_capture')).toHaveLength(0);
  expect((await h.runtime.getSnapshot()).state).toBe('recording');
  h.release();
  h.clock.advance(80);
  expect(h.modes).toEqual([false, false, true]);
  transcription.resolve();
  await waitFor(() => h.commands('report_transcript_delivery').length === 1);
  expect(h.commands('cancel_transcription_job')).toHaveLength(0);
  await h.runtime.dispose();
});

async function createHotkeyRuntime(config = {}) {
  const boundary = createBoundary();
  const listeners = new Map();
  const shortcuts = new Map();
  const clock = createClock();
  const delivered = [];
  const modes = [];
  let pasteCount = 0;
  let fnListener;
  let sequence = 1;
  let activeIntent;
  let lateStop;
  let captureSequence = 0;
  let current = { sequence, snapshot: echoSnapshot('idle') };
  let settings = { revision: 1, settings: { ...defaultSettings, pushToTalkShortcut: 'Fn' } };
  const commands = (name) => boundary.calls.filter(([command]) => command === name);
  const publish = (state) => {
    current = { sequence: ++sequence, snapshot: echoSnapshot(state) };
    listeners.get('echo-desktop-state')?.({ payload: current });
    return current;
  };
  boundary.listen = async (event, listener) => {
    listeners.set(event, listener);
    return () => listeners.delete(event);
  };
  boundary.invoke = async (command, args) => {
    boundary.calls.push([command, args]);
    if (command === 'get_desktop_runtime_snapshot') return current;
    if (command === 'get_persisted_settings') return settings;
    if (command === 'update_persisted_settings') {
      const previous = settings;
      settings = { revision: settings.revision + 1, settings: { ...settings.settings, ...args.patch } };
      return { previous, current: settings, changedFields: Object.keys(args.patch) };
    }
    if (command === 'start_audio_capture') {
      if (lateStop && commands(command).length === 2) {
        current = lateStop;
        listeners.get('echo-desktop-state')?.({ payload: lateStop });
      }
      const intent = args.request.intentId;
      activeIntent = intent;
      publish('requesting_microphone');
      try {
        if (commands(command).length === 1 && config.starting) await config.starting.promise;
      } catch (error) {
        if (activeIntent === intent) publish('error');
        throw error;
      }
      if (activeIntent !== intent) throw captureError('capture_cancelled');
      publish('recording');
      if (config.started) await config.started.promise;
      return session(intent, `session-${intent}`);
    }
    if (command === 'cancel_audio_capture') {
      if (activeIntent === args.request.intentId) { activeIntent = undefined; publish('idle'); }
      return;
    }
    if (command === 'stop_audio_capture') {
      if (config.stopping) await config.stopping.promise;
      activeIntent = undefined;
      if (config.lateStop) {
        lateStop = { sequence: ++sequence, snapshot: echoSnapshot('stopping') };
      } else {
        publish('stopping');
      }
      return artifact(args.request.sessionId);
    }
    if (command === 'deliver_transcript') {
      delivered.push(args.request.text);
      if (args.request.pasteAfterTranscription) pasteCount += 1;
      return args.request.pasteAfterTranscription ? 'pasted' : 'copied';
    }
    if (command === 'transcribe_audio_artifact') {
      if (config.transcription) await config.transcription.promise;
      return {
        delivery: { pasteAfterTranscription: true, restoreClipboard: false },
        jobSequence: 1,
        persistence: { saved: true },
        snapshot: activeIntent ? current : publish('done'),
        transcript: { id: 'transcript', text: 'hello', source: 'desktop', ...config.transcript }
      };
    }
    return boundary.responses.get(command);
  };
  const runtime = createRuntime(boundary, {
    hotkeyOwner: true,
    hotkeyClock: clock,
    hotkeyBoundary: {
      listenFn: async (listener) => { fnListener = listener; return () => undefined; },
      register: async (shortcut, listener) => { shortcuts.set(shortcut, listener); },
      unregister: async (shortcut) => { shortcuts.delete(shortcut); },
      setFnCaptureEnabled: async () => undefined
    }
  });
  runtime.subscribe((event) => { if (event.type === 'hands_free_changed') modes.push(event.active); });
  await runtime.getSnapshot();
  const press = () => fnListener({ state: 'pressed' });
  const release = () => fnListener({ state: 'released' });
  return {
    runtime, clock, commands, delivered, modes, publish, press, release, shortcuts,
    emitCapture(event) { listeners.get('echo-audio-capture')?.({ payload: event }); },
    emitPartial(event) { listeners.get('echo-partial-transcript')?.({ payload: event }); },
    captureEvent(patch = {}) { return { sequence: ++captureSequence, phase: 'recording', outcome: 'recording_progress',
      intentId: activeIntent, sessionId: `session-${activeIntent}`, purpose: 'push_to_talk', artifactId: null,
      elapsedMs: 600_000, limitMs: 600_000, error: null, ...patch }; },
    pastes: () => pasteCount,
    snapshot: () => current.snapshot,
    async latch() {
      const count = commands('start_audio_capture').length;
      press();
      await waitFor(() => commands('start_audio_capture').length === count + 1);
      await waitFor(() => current.snapshot.state === 'recording');
      release();
      clock.advance(80);
      await flush();
    }
  };
}

function createClock() {
  let time = 0;
  const timers = new Set();
  const callbacks = [];
  return {
    callbacks,
    now: () => time,
    schedule(callback, delay) {
      const timer = { at: time + delay, callback };
      timers.add(timer);
      callbacks.push(callback);
      return () => timers.delete(timer);
    },
    advance(duration) {
      const target = time + duration;
      for (;;) {
        const next = [...timers].sort((a, b) => a.at - b.at)[0];
        if (!next || next.at > target) break;
        time = next.at;
        timers.delete(next);
        next.callback();
      }
      time = target;
    }
  };
}

async function flush() {
  for (let i = 0; i < 100; i += 1) await Promise.resolve();
}

test('capture announces the original intent before native start', async () => {
  for (const failure of [false, true]) {
    const seen = [];
    const controller = createCaptureIntentController({
      client: createClient({ start: async () => {
        expect(seen).toEqual(['begin:intent']);
        if (failure) throw captureError('microphone_unavailable');
        return session('intent', 'session');
      } }),
      createIntentId: () => 'intent',
      onIntent: (id) => seen.push(`begin:${id}`),
      purpose: 'push_to_talk'
    });
    if (failure) await expect(controller.press()).rejects.toMatchObject({ code: 'microphone_unavailable' });
    else { await controller.press(); await controller.cancel(); }
    expect(seen).toEqual(['begin:intent']);
  }
});

function createBoundary() {
  return {
    calls: [],
    listener: undefined,
    responses: new Map(),
    unlistenCalls: 0,
    listenedEvents: [],
    async invoke(command, argumentsValue) {
      this.calls.push([command, argumentsValue]);
      return await this.responses.get(command);
    },
    async listen(event, listener) {
      this.listenedEvents.push(event);
      this.listener = listener;
      return () => { this.unlistenCalls += 1; };
    }
  };
}

function createClient(overrides = {}) {
  return {
    start: async () => session('default', 'default-session'),
    stop: async (sessionId) => artifact(sessionId),
    cancel: async () => undefined,
    listen: async () => () => undefined,
    ...overrides
  };
}

function session(intentId, sessionId) {
  return { channelCount: 1, intentId, phase: 'recording', purpose: 'push_to_talk', sampleFormat: 'f32', sampleRate: 48_000, sessionId };
}

function artifact(sessionId) {
  return { artifactId: 'artifact-a', byteSize: 1024, channelCount: 1, durationMs: 500, mimeType: 'audio/wav', purpose: 'push_to_talk', sampleRate: 48_000, sessionId };
}

function captureError(code) {
  return { code, phase: 'permission_pending', retryable: true, stage: 'start' };
}

function echoSnapshot(state) {
  return { activeProviderId: 'groq-whisper-turbo', activeShortcut: 'Fn', mode: 'desktop', state };
}

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

// Keep: automatic and manual stop share one consumable result through the real runtime.
for (const manualFirst of [false, true]) {
  test(`automatic completion delivers once with manual race (manual first: ${manualFirst})`, async () => {
    const stopping = deferred();
    const h = await createHotkeyRuntime({ stopping });
    await h.latch();
    const manual = manualFirst ? h.runtime.stopRecording() : undefined;
    const terminal = h.captureEvent({ phase: 'idle', outcome: 'artifact_created', artifactId: 'artifact' });
    h.emitCapture(terminal);
    const concurrent = h.runtime.stopRecording();
    stopping.resolve();
    await Promise.all([manual, concurrent]);
    await waitFor(() => h.commands('report_transcript_delivery').length >= 1);
    expect(h.commands('stop_audio_capture')).toHaveLength(1);
    expect(h.commands('transcribe_audio_artifact')).toHaveLength(1);
    expect(h.delivered).toEqual(['hello']);
    expect(h.commands('cancel_audio_capture')).toHaveLength(0);
    h.press();
    await waitFor(() => h.commands('start_audio_capture').length === 2);
    h.emitCapture({ ...terminal, sequence: terminal.sequence + 1 });
    await flush();
    expect(h.commands('cancel_audio_capture')).toHaveLength(0);
    expect(h.commands('transcribe_audio_artifact')).toHaveLength(1);
    await h.runtime.dispose();
  });
}

test('native progress is typed, identity scoped and uses native elapsed time', async () => {
  const h = await createHotkeyRuntime();
  const progress = [];
  h.runtime.subscribe(event => { if (event.type === 'recording_progress') progress.push(event); });
  await h.latch();
  const event = h.captureEvent({ elapsedMs: 540_250, limitMs: 600_000 });
  h.emitCapture(event);
  h.emitCapture({ ...event, elapsedMs: 1 });
  h.emitCapture({ ...event, sequence: event.sequence + 1, intentId: 'foreign' });
  expect(progress.at(-1)).toEqual({ type: 'recording_progress', intentId: event.intentId,
    sessionId: event.sessionId, elapsedMs: 540_250, limitMs: 600_000 });
  expect(progress.filter(value => value.elapsedMs === 1)).toHaveLength(0);
  await h.runtime.dispose();
});

test('automatic completion alone transcribes and delivers retained audio', async () => {
  const h = await createHotkeyRuntime();
  await h.latch();
  h.publish('idle');
  h.emitCapture(h.captureEvent({ phase: 'idle', outcome: 'artifact_created', artifactId: 'artifact' }));
  await waitFor(() => h.commands('report_transcript_delivery').length === 1);
  expect(h.commands('transcribe_audio_artifact')).toHaveLength(1);
  expect(h.delivered).toEqual(['hello']);
  expect(h.commands('cancel_audio_capture')).toHaveLength(0);
  await h.runtime.dispose();
});

test('automatic completion of a long hold ignores repeats and its eventual release', async () => {
  const h = await createHotkeyRuntime();
  h.press();
  await waitFor(() => h.snapshot().state === 'recording');
  h.clock.advance(600_000);
  h.emitCapture(h.captureEvent({ phase: 'idle', outcome: 'artifact_created', artifactId: 'artifact' }));
  h.press(); h.press();
  await waitFor(() => h.commands('report_transcript_delivery').length === 1);
  h.release();
  h.clock.advance(1000);
  await flush();
  expect(h.commands('start_audio_capture')).toHaveLength(1);
  expect(h.delivered).toEqual(['hello']);
  h.press();
  await waitFor(() => h.commands('start_audio_capture').length === 2);
  await h.runtime.dispose();
});

// Keep: state envelopes alone and foreign terminal identities cannot cancel current audio.
test('only a matching native terminal event retires a settled capture', async () => {
  const h = await createHotkeyRuntime();
  await h.latch();
  h.publish('idle');
  h.emitCapture(h.captureEvent({ intentId: 'old', phase: 'idle', error: 'stream_backend_error' }));
  h.emitCapture(h.captureEvent({ sessionId: 'old-session', phase: 'idle', error: 'stream_backend_error' }));
  await flush();
  expect(h.commands('cancel_audio_capture')).toHaveLength(0);
  h.emitCapture(h.captureEvent({ phase: 'idle', error: 'stream_backend_error' }));
  await waitFor(() => h.commands('cancel_audio_capture').length === 1);
  await h.runtime.dispose();
});

// Keep: native errors can precede the rejected start command, without cancelling it.
for (const outcome of ['start_failed', 'first_callback_timeout']) {
  for (const code of ['artifact_capacity_reached', 'microphone_permission_denied', 'microphone_unavailable']) {
    test(`start rejection surfaces once after ${outcome}: ${code}`, async () => {
      const starting = deferred();
      const h = await createHotkeyRuntime({ starting });
      const errors = [];
      h.runtime.subscribe(event => { if (event.type === 'error') errors.push(event); });
      h.press();
      await waitFor(() => h.commands('start_audio_capture').length === 1);
      await flush();
      h.emitCapture(h.captureEvent({ phase: 'idle', outcome, error: code, elapsedMs: 0 }));
      await flush();
      expect(h.commands('cancel_audio_capture')).toHaveLength(0);
      starting.reject({ code, phase: 'opening', retryable: true, stage: 'start' });
      await flush();
      expect(errors).toHaveLength(1);
      expect(h.commands('transcribe_audio_artifact')).toHaveLength(0);
      h.release();
      h.clock.advance(80);
      h.press();
      await waitFor(() => h.commands('start_audio_capture').length === 2);
      await h.runtime.dispose();
    });
  }
}

// Keep: native terminal stream errors can precede the start command response.
test('terminal stream failure matches while native start response is pending', async () => {
  let resolveStart;
  const controller = createCaptureIntentController({
    purpose: 'push_to_talk', createIntentId: () => 'pending',
    client: { start: () => new Promise((resolve) => { resolveStart = resolve; }),
      stop: async () => artifact('session-pending'), cancel: async () => undefined }
  });
  const starting = controller.press();
  await Promise.resolve();
  const event = { phase: 'idle', error: 'stream_failed', outcome: 'stream_failed', intentId: 'pending', sessionId: 'session-pending' };
  expect(controller.matches(event)).toBe(true);
  expect(controller.matches({ ...event, outcome: 'start_failed' })).toBe(false);
  expect(controller.matches({ ...event, outcome: 'first_callback_timeout' })).toBe(false);
  resolveStart(session('pending', 'session-pending'));
  await starting;
  await controller.dispose();
});

// Keep: marker-only rows reach History without a clipboard or paste request.
test('all-failed marker-only completion never automatically delivers', async () => {
  const h = await createHotkeyRuntime({ transcript: { text: '[Transcription gap] [Transcription gap]', errorCode: 'provider_failed', insertionMode: 'failed' } });
  await h.latch();
  await h.runtime.stopRecording();
  expect(h.commands('transcribe_audio_artifact')).toHaveLength(1);
  expect(h.commands('deliver_transcript')).toHaveLength(0);
  expect(h.commands('report_transcript_delivery')).toHaveLength(0);
  expect(h.delivered).toEqual([]);
  await h.runtime.dispose();
});

// Keep: automatic stop clears displayed words while tail status remains observable.
test('automatic stop accepts retry status until final transcription completes', async () => {
  const transcription = deferred();
  const h = await createHotkeyRuntime({ transcription });
  const partials = [];
  h.runtime.subscribe(event => { if (event.type === 'partial_transcript_changed') partials.push(event.projection); });
  await h.latch();
  const terminal = h.captureEvent({ phase: 'idle', outcome: 'artifact_created', artifactId: 'artifact' });
  h.emitCapture(terminal);
  await waitFor(() => h.commands('transcribe_audio_artifact').length === 1);
  h.emitPartial({ dictationId: terminal.intentId, ordinal: 0, text: '', status: 'retrying' });
  expect(partials.at(-1)?.status).toBe('retrying');
  transcription.resolve();
  await waitFor(() => h.commands('report_transcript_delivery').length === 1);
  await h.runtime.dispose();
});

// Keep: recognized speech that happens to equal the marker is still delivered.
test('partial speech equal to the marker still delivers when completion is manual', async () => {
  const h = await createHotkeyRuntime({ transcript: { text: '[Transcription gap]', errorCode: 'provider_failed', insertionMode: 'manual' } });
  await h.latch();
  await h.runtime.stopRecording();
  expect(h.commands('deliver_transcript')).toHaveLength(1);
  await h.runtime.dispose();
});
