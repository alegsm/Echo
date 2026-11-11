// @ts-nocheck
import { expect, test } from 'bun:test';
import { createDesktopRuntime } from '../src/lib/runtime/desktop/desktop-runtime';
const clipboardWrites = [];
const providerRequests = [];
const calls = [];
let pasteCalls = 0;
let pasteFailure = false;
let pendingCaptureStart;
let stateSequence = 1;
let snapshotState = 'idle';
const eventListeners = new Map();
const listenerSets = new Map();
let initializationFailure = false;
let credentialPresent = false;
let credentialFailure = false;
let settingsRevision = 1;
let committedSettings = settings();
let searchOutcome = 'not_found';
let settingsGate;

const boundary = {
  async invoke(command, argumentsValue) {
    calls.push([command, argumentsValue]);
    if (['legacy_migration_receipt', 'import_legacy_state', 'mark_legacy_migration_complete'].includes(command)) {
      throw new Error('Retired import command invoked');
    }
    if (command === 'get_persisted_settings' && initializationFailure) throw new Error('Native settings unavailable');
    if (command === 'provider_api_key_presence') return { 'groq-whisper-turbo': credentialPresent };
    if (command === 'set_provider_api_key' || command === 'delete_provider_api_key') {
      if (credentialFailure) throw new Error('Keychain operation failed.');
      credentialPresent = command === 'set_provider_api_key';
      eventListeners.get('echo-credentials-updated')?.({ payload: {
        action: credentialPresent ? 'set' : 'delete', providerId: argumentsValue.providerId
      } });
    }
    if (command === 'find_provider_api_key') {
      if (searchOutcome === 'found') settingsRevision += 1;
      return { outcome: searchOutcome };
    }
    if (command === 'get_persisted_settings') {
      const gate = settingsGate;
      if (gate) await gate.promise;
      return { revision: settingsRevision, settings: committedSettings };
    }
    if (command === 'get_desktop_runtime_snapshot') return envelope(snapshotState);
    if (command === 'start_audio_capture') {
      if (pendingCaptureStart) return pendingCaptureStart.promise;
      const intentId = argumentsValue.request.intentId;
      return session(intentId);
    }
    if (command === 'stop_audio_capture') {
      const request = argumentsValue.request;
      return artifact(request.sessionId);
    }
    if (command === 'transcribe_audio_artifact') {
      const request = deferred();
      providerRequests.push({ proof: argumentsValue.request, request });
      return request.promise;
    }
    if (command === 'deliver_transcript') {
      clipboardWrites.push(argumentsValue.request.text);
      if (!argumentsValue.request.pasteAfterTranscription) return 'copied';
      pasteCalls += 1;
      if (pasteFailure) throw { code: 'insertion_failed' };
      return 'pasted';
    }
    if (command === 'copy_transcript') clipboardWrites.push(argumentsValue.request.text);
    return undefined;
  },
  listen: async (name, listener) => {
    const listeners = listenerSets.get(name) ?? new Set();
    listenerSets.set(name, listeners);
    listeners.add(listener);
    eventListeners.set(name, (event) => { for (const callback of listeners) callback(event); });
    return () => {
      listeners.delete(listener);
      if (!listeners.size) { listenerSets.delete(name); eventListeners.delete(name); }
    };
  }
};
function createRuntime({ hotkeyOwner = false, hotkeyBoundary } = {}) {
  return createDesktopRuntime({ boundary, hotkeyOwner,
    hotkeyBoundary: hotkeyBoundary ?? { listenFn: async () => () => {}, register: async () => {}, unregister: async () => {}, setFnCaptureEnabled: async () => {} },
    hotkeyClock: { now: () => 0, schedule() { throw new Error('Unexpected timer'); } },
    syncDiagnostics: async () => {}
  });
}
test('an older job failure cannot surface while a newer capture is still held', async () => {
  const firstRequest = providerRequests.length;
  const runtime = createRuntime({ hotkeyOwner: false });
  const errors = [];
  const unsubscribe = runtime.subscribe((event) => {
    if (event.type === 'error') errors.push(event.code);
  });
  await runtime.startRecording();
  const stoppingFirst = runtime.stopRecording();
  await waitFor(() => providerRequests.length === firstRequest + 1);
  await runtime.startRecording();
  snapshotState = 'recording';
  stateSequence += 1;
  providerRequests[firstRequest].request.reject(sttError('provider_failed'));
  await stoppingFirst;

  expect(errors).toEqual([]);
  expect((await runtime.getSnapshot()).state).toBe('recording');
  unsubscribe();
  await runtime.cancelRecording();
  await runtime.dispose();
  snapshotState = 'idle';
  stateSequence += 1;
});


test('an older job failure cannot surface while a newer native start is pending', async () => {
  const firstRequest = providerRequests.length;
  const runtime = createRuntime({ hotkeyOwner: false });
  const errors = [];
  const unsubscribe = runtime.subscribe((event) => {
    if (event.type === 'error') errors.push(event.code);
  });
  await runtime.startRecording();
  const stoppingFirst = runtime.stopRecording();
  await waitFor(() => providerRequests.length === firstRequest + 1);
  const startsBefore = calls.filter(([command]) => command === 'start_audio_capture').length;
  pendingCaptureStart = deferred();
  const startingSecond = runtime.startRecording();
  await waitFor(() => {
    return calls.filter(([command]) => command === 'start_audio_capture').length > startsBefore;
  });
  providerRequests[firstRequest].request.reject(sttError('provider_failed'));
  await stoppingFirst;

  expect(errors).toEqual([]);
  pendingCaptureStart.reject(captureError('capture_cancelled'));
  await startingSecond;
  pendingCaptureStart = undefined;
  unsubscribe();
  await runtime.dispose();
});


test('a duplicate stop cannot suppress the current provider failure', async () => {
  const requestIndex = providerRequests.length;
  const runtime = createRuntime({ hotkeyOwner: false });
  const errors = [];
  const unsubscribe = runtime.subscribe((event) => {
    if (event.type === 'error') errors.push(event.code);
  });
  await runtime.startRecording();
  const stopping = runtime.stopRecording();
  await waitFor(() => providerRequests.length === requestIndex + 1);
  await runtime.stopRecording();
  snapshotState = 'error';
  stateSequence += 1;
  providerRequests[requestIndex].request.reject(sttError('provider_failed'));
  await stopping;

  expect(errors).toEqual(['provider_failed']);
  unsubscribe();
  await runtime.dispose();
  snapshotState = 'idle';
  stateSequence += 1;
});


test('a current failure refreshes authoritative error state before emitting its message', async () => {
  const requestIndex = providerRequests.length;
  const runtime = createRuntime({ hotkeyOwner: false });
  const terminalEvents = [];
  const unsubscribe = runtime.subscribe((event) => {
    if (event.type === 'state_changed' || event.type === 'error') terminalEvents.push(event);
  });
  await runtime.startRecording();
  const stopping = runtime.stopRecording();
  await waitFor(() => providerRequests.length === requestIndex + 1);
  snapshotState = 'error';
  stateSequence += 1;
  providerRequests[requestIndex].request.reject(sttError('provider_failed'));
  await stopping;

  expect(terminalEvents.slice(-2).map((event) => event.type)).toEqual(['state_changed', 'error']);
  expect(terminalEvents.at(-2)?.snapshot.state).toBe('error');
  expect(terminalEvents.at(-1)?.code).toBe('provider_failed');
  unsubscribe();
  await runtime.dispose();
  snapshotState = 'idle';
  stateSequence += 1;
});


test('a current paste failure remains a visible typed error', async () => {
  const requestIndex = providerRequests.length;
  const runtime = createRuntime({ hotkeyOwner: false });
  const errors = [];
  const unsubscribe = runtime.subscribe((event) => {
    if (event.type === 'error') errors.push(event.code);
  });
  await runtime.startRecording();
  const stopping = runtime.stopRecording();
  await waitFor(() => providerRequests.length === requestIndex + 1);
  pasteFailure = true;
  providerRequests[requestIndex].request.resolve(
    completion(providerRequests[requestIndex].proof, 'paste should fail')
  );
  await stopping;

  expect(errors).toContain('insertion_failed');
  expect(calls.findLast(([command]) => command === 'report_transcript_delivery')[1].request)
    .toMatchObject({ insertionMode: 'failed', errorCode: 'insertion_failed' });
  pasteFailure = false;
  unsubscribe();
  await runtime.dispose();
});


test('native initialization failure prevents capture and provider requests', async () => {
  initializationFailure = true;
  const before = calls.length;
  const runtime = createRuntime({ hotkeyOwner: false });
  try {
    await expect(runtime.getSnapshot()).rejects.toThrow('Native settings unavailable');
    await expect(runtime.startRecording()).rejects.toThrow('Native settings unavailable');
    await expect(runtime.probeProviderKey({ providerId: 'groq-whisper-turbo' })).rejects.toThrow('Native settings unavailable');
    expect(calls.slice(before).map(([name]) => name)).not.toContain('start_audio_capture');
    expect(calls.slice(before).map(([name]) => name)).not.toContain('probe_provider');
  } finally {
    initializationFailure = false;
    await runtime.dispose();
  }
});


test('backend credential events refresh dashboard presence once per successful set and delete', async () => {
  const dashboard = createRuntime({ hotkeyOwner: false });
  const presenceRefreshes = [];
  const notifications = [];
  dashboard.subscribe((event) => {
    if (event.type === 'credentials_updated') {
      presenceRefreshes.push(dashboard.getProviderApiKeyPresence());
      notifications.push(event);
    }
  });
  try {
    await dashboard.getSnapshot();
    await dashboard.setProviderApiKey('groq-whisper-turbo', '<redacted>');
    expect(notifications).toHaveLength(1);
    expect(await presenceRefreshes[0]).toEqual({ 'groq-whisper-turbo': true });
    await dashboard.deleteProviderApiKey('groq-whisper-turbo');
    expect(notifications).toHaveLength(2);
    expect(await presenceRefreshes[1]).toEqual({ 'groq-whisper-turbo': false });
    credentialFailure = true;
    await expect(dashboard.setProviderApiKey('groq-whisper-turbo', '<redacted>')).rejects.toThrow('Keychain');
    await expect(dashboard.deleteProviderApiKey('groq-whisper-turbo')).rejects.toThrow('Keychain');
    expect(notifications).toHaveLength(2);
  } finally {
    credentialFailure = false;
    credentialPresent = false;
    await dashboard.dispose();
  }
});

// Keep: only adoption reloads settings; a failed reload never discards the native outcome.
test.each(['found', 'not_found', 'invalid', 'unavailable', 'already_saved', 'save_failed'])('saved-key search %s keeps its outcome across the settings reload', async (outcome) => {
  const dashboard = createRuntime({ hotkeyOwner: false });
  const notifications = [];
  dashboard.subscribe((event) => { if (event.type === 'credentials_updated') notifications.push(event); });
  try {
    await dashboard.getSnapshot();
    searchOutcome = outcome;
    const reloads = outcome === 'found' ? [['get_persisted_settings', undefined]] : [];
    let before = calls.length;
    settingsGate = deferred();
    let settled = false;
    const searching = dashboard.findProviderApiKey('groq-whisper-turbo').then((value) => { settled = true; return value; });
    if (outcome === 'found') {
      await waitFor(() => calls.slice(before).some(([command]) => command === 'get_persisted_settings'));
      for (let i = 0; i < 10; i++) await Promise.resolve();
      expect(settled).toBe(false);
    }
    settingsGate.resolve();
    settingsGate = undefined;
    expect(await searching).toBe(outcome);
    expect(calls.slice(before)).toEqual([['find_provider_api_key', { providerId: 'groq-whisper-turbo' }], ...reloads]);
    initializationFailure = true;
    before = calls.length;
    expect(await dashboard.findProviderApiKey('groq-whisper-turbo')).toBe(outcome);
    expect(calls.slice(before)).toEqual([['find_provider_api_key', { providerId: 'groq-whisper-turbo' }], ...reloads]);
    expect(notifications).toEqual([]);
  } finally {
    initializationFailure = false;
    settingsGate?.resolve();
    settingsGate = undefined;
    searchOutcome = 'not_found';
    await dashboard.dispose();
  }
});

function settings() {
  return {
    pushToTalkShortcut: 'Fn',
    fallbackShortcut: 'Alt+Space',
    providerId: 'groq-whisper-turbo',
    providerKeyStatuses: { 'groq-whisper-turbo': 'valid' },
    phrasebookTerms: [],
    pasteAfterTranscription: true,
    restoreClipboard: false,
    debugLoggingEnabled: false,
    historyRetentionDays: 30
  };
}

function session(intentId) {
  return { channelCount: 1, intentId, phase: 'recording', purpose: 'push_to_talk', sampleFormat: 'f32', sampleRate: 48_000, sessionId: `session-${intentId}` };
}

function artifact(sessionId) {
  return { artifactId: `artifact-${sessionId}`, byteSize: 1024, channelCount: 1, durationMs: 500, mimeType: 'audio/wav', purpose: 'push_to_talk', sampleRate: 48_000, sessionId };
}

function completion(proof, text) {
  const transcript = {
    id: `transcript-${proof.intentId}`,
    text,
    createdAt: '2026-09-09T00:00:00.000Z',
    durationMs: 500,
    providerId: 'groq-whisper-turbo',
    timings: { recordingMs: 500, blobReadyMs: 0, uploadMs: 0, stopToTranscriptMs: 20 },
    audio: { mimeType: 'audio/wav', byteSize: 1024 },
    insertionMode: 'manual',
    source: 'desktop'
  };
  return {
    delivery: { pasteAfterTranscription: true, restoreClipboard: false },
    jobSequence: ++stateSequence,
    persistence: { saved: true },
    snapshot: { sequence: stateSequence, snapshot: { ...envelope('done').snapshot, lastTranscript: transcript } },
    transcript
  };
}

function envelope(state) {
  return { sequence: stateSequence, snapshot: { activeProviderId: 'groq-whisper-turbo', activeShortcut: 'Fn', mode: 'desktop', state } };
}

function captureError(code) {
  return { activePurpose: null, code, phase: 'permission_pending', retryable: true, stage: 'start' };
}

function sttError(code) {
  return { code, message: 'safe provider failure', retryable: true, stage: 'provider' };
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((resolveValue, rejectValue) => { resolve = resolveValue; reject = rejectValue; });
  return { promise, reject, resolve };
}

async function waitFor(predicate) {
  for (let attempt = 0; attempt < 30; attempt += 1) {
    if (predicate()) return;
    await Promise.resolve();
  }
  throw new Error('Condition was not met');
}

// Keep: the public runtime distinguishes rejected credentials from transient failures.
test.each([
  ['provider_credential_rejected', 'missing_provider_key'],
  ['missing_provider_key', 'missing_provider_key'],
  ['provider_timeout', 'provider_timeout'],
  ['provider_failed', 'provider_failed']
])('native %s retains its public classification', async (nativeCode, publicCode) => {
  const { runtimeErrorCode } = await import('../src/lib/runtime/desktop/desktop-error');
  expect(runtimeErrorCode(sttError(nativeCode))).toBe(publicCode);
});


// Keep: the public entry point rejects non-desktop environments before native initialization.
test.each(['window', 'internals'])('the runtime factory refuses missing %s', async missing => {
  const { createEchoRuntime } = await import('../src/lib/runtime/runtime');
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'window');
  try {
    if (missing === 'window') Reflect.deleteProperty(globalThis, 'window');
    else Object.defineProperty(globalThis, 'window', { configurable: true, value: {} });
    expect(() => createEchoRuntime()).toThrow('Echo requires the Tauri desktop app.');
  } finally {
    if (descriptor) Object.defineProperty(globalThis, 'window', descriptor);
    else Reflect.deleteProperty(globalThis, 'window');
  }
});

// Keep: lifecycle reconfiguration errors describe the actual recovery action.
test.each(['missing-fallback', 'unregister'])('shortcut %s failure has distinct runtime copy', async (failure) => {
  const previousSettings = committedSettings;
  committedSettings = { ...settings(), pushToTalkShortcut: 'Control+Windows', fallbackShortcut: failure === 'missing-fallback' ? '' : 'Control+Shift+Space' };
  let available = failure === 'missing-fallback';
  let listener;
  const runtime = createRuntime({ hotkeyOwner: true, hotkeyBoundary: {
    listenFn: async (callback) => { listener = callback; return () => {}; },
    register: async () => {},
    unregister: async () => { if (failure === 'unregister') throw new Error('OS failure'); },
    setFnCaptureEnabled: async () => available
  } });
  const errors = [];
  runtime.subscribe(event => { if (event.type === 'error') errors.push(event.message); });
  try {
    await runtime.getSnapshot();
    await new Promise(resolve => setTimeout(resolve, 0));
    available = !available;
    listener({ state: available ? 'available' : 'unavailable' });
    await waitFor(() => errors.length > 0);
    expect(errors.at(-1)).toBe(failure === 'missing-fallback'
      ? 'Native shortcut unavailable. Choose a fallback shortcut in Settings.'
      : 'The previous shortcut could not be removed. Try changing the shortcut again.');
  } finally {
    await runtime.dispose().catch(() => {});
    committedSettings = previousSettings;
  }
});
