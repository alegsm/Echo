import { expect, test } from 'bun:test';
import { defaultSettings } from '../src/lib/core/settings';
import type { EchoRuntimeEvent, EchoSnapshot } from '../src/lib/core/types';
import type { TranscriptRecord } from '../src/lib/core/transcript';
import { createDesktopRuntime } from '../src/lib/runtime/desktop/desktop-runtime';
import type { NativeCaptureBoundary } from '../src/lib/runtime/desktop/native-capture-client';

// Keep: real runtime composition from the injected native boundary through delivery and disposal.
test.each([false, true])('desktop wiring smoke (overlay owner: %s)', async (hotkeyOwner) => {
  const calls: Array<[string, Record<string, unknown> | undefined]> = [];
  const listeners = new Map<string, (event: { payload: unknown }) => void>();
  const events: EchoRuntimeEvent[] = [];
  const diagnostics: boolean[] = [];
  let revision = 1, sequence = 1;
  let settings = { ...defaultSettings };
  let snapshot: EchoSnapshot = { mode: 'desktop', state: 'idle', activeProviderId: settings.providerId, activeShortcut: 'Fn' };
  let intentId = '';
  const publish = (state: EchoSnapshot['state']) => {
    snapshot = { ...snapshot, state };
    const envelope = { sequence: ++sequence, snapshot };
    listeners.get('echo-desktop-state')?.({ payload: envelope });
    return envelope;
  };
  const transcript: TranscriptRecord = {
    id: 'native-record', text: 'Dictated text', createdAt: '2026-11-08T00:00:00Z', durationMs: 500,
    providerId: settings.providerId, source: 'desktop', insertionMode: 'manual',
    timings: { recordingMs: 500, blobReadyMs: 0, uploadMs: 0, stopToTranscriptMs: 20 },
    audio: { mimeType: 'audio/wav', byteSize: 1024 }
  };
  const boundary: NativeCaptureBoundary = {
    async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
      calls.push([command, args]);
      let result: unknown;
      const request = args?.request as Record<string, unknown> | undefined;
      switch (command) {
        case 'get_persisted_settings': result = { revision, settings }; break;
        case 'get_desktop_runtime_snapshot': result = { sequence, snapshot }; break;
        case 'start_audio_capture':
          intentId = String(request!.intentId);
          if (hotkeyOwner) expect(listeners.has('echo-partial-transcript')).toBe(true);
          publish('recording');
          result = { intentId, sessionId: 'native-session', purpose: 'push_to_talk', phase: 'recording', channelCount: 1, sampleRate: 48000, sampleFormat: 'f32' };
          break;
        case 'stop_audio_capture':
          expect(request).toEqual({ intentId, sessionId: 'native-session' });
          publish('stopping');
          result = { artifactId: 'native-artifact', sessionId: 'native-session', purpose: 'push_to_talk', mimeType: 'audio/wav', byteSize: 1024, channelCount: 1, durationMs: 500, sampleRate: 48000 };
          break;
        case 'transcribe_audio_artifact':
          expect(request).toEqual({ intentId, sessionId: 'native-session', artifactId: 'native-artifact' });
          result = { transcript, delivery: { pasteAfterTranscription: true, restoreClipboard: false }, persistence: { saved: true }, jobSequence: 1, snapshot: publish('done') };
          break;
        case 'deliver_transcript': result = 'pasted'; break;
        case 'report_transcript_delivery': result = { historyRevision: 1, transcript: { record: transcript, revision: 1 } }; break;
        case 'provider_api_key_presence': result = { [settings.providerId]: true }; break;
        case 'cancel_audio_capture': publish('idle'); break;
        default: throw new Error(`Unexpected native command: ${command}`);
      }
      return result as T;
    },
    async listen<T>(name: string, listener: (event: { payload: T }) => void) {
      listeners.set(name, ({ payload }) => listener({ payload: payload as T }));
      return () => { listeners.delete(name); };
    }
  };
  const storageDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  let storageReads = 0;
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, get() { storageReads++; throw new Error('Legacy storage must not be read'); } });
  const runtime = createDesktopRuntime({ boundary, hotkeyOwner,
    hotkeyBoundary: { listenFn: async () => () => {}, register: async () => {}, unregister: async () => {}, setFnCaptureEnabled: async () => {} },
    // No wall-clock scheduling is allowed in this smoke path.
    hotkeyClock: { now: () => 0, schedule() { throw new Error('Unexpected hotkey timer'); } },
    syncDiagnostics: async enabled => { diagnostics.push(enabled); }
  });
  const unsubscribe = runtime.subscribe(event => events.push(event));
  try {
    expect((await runtime.getSnapshot()).state).toBe('idle');
    expect(runtime.kind).toBe('tauri');
    expect(runtime.mode).toBe('desktop');
    expect((await runtime.getSettings()).providerId).toBe(settings.providerId);
    expect(diagnostics).toContain(false);
    expect(listeners.has('echo-partial-transcript')).toBe(hotkeyOwner);
    expect((await runtime.startRecording()).state).toBe('recording');
    listeners.get('echo-partial-transcript')?.({ payload: { dictationId: intentId, ordinal: 0, text: 'partial' } });
    expect(events.some(event => event.type === 'partial_transcript_changed' && event.projection.text === 'partial')).toBe(hotkeyOwner);
    expect(calls.some(([name]) => name === 'deliver_transcript')).toBe(false);
    await runtime.stopRecording();
    expect(events.findLast(event => event.type === 'transcript_completed')).toMatchObject({ transcript, dictationId: intentId });
    expect(calls.filter(([name]) => name === 'deliver_transcript')).toEqual([
      ['deliver_transcript', { request: { text: transcript.text, pasteAfterTranscription: true, restoreClipboard: false } }]
    ]);
    expect(calls.find(([name]) => name === 'report_transcript_delivery')?.[1]?.request)
      .toEqual({ intentId, sessionId: 'native-session', transcriptId: transcript.id, insertionMode: 'pasted' });
    listeners.get('echo-credentials-updated')!({ payload: { providerId: settings.providerId, action: 'set' } });
    expect(events.at(-1)?.type).toBe('credentials_updated');
    expect((await runtime.getProviderApiKeyPresence())[settings.providerId]).toBe(true);
    settings = { ...settings, language: 'es' };
    listeners.get('echo-settings-committed')!({ payload: { revision: ++revision, changedFields: ['language'] } });
    for (let i = 0; i < 20; i++) await Promise.resolve();
    expect((await runtime.getSettings()).language).toBe('es');
    expect(events.some(event => event.type === 'settings_changed')).toBe(true);
    expect(storageReads).toBe(0);
  } finally {
    unsubscribe();
    await runtime.dispose();
    if (storageDescriptor) Object.defineProperty(globalThis, 'localStorage', storageDescriptor);
    else Reflect.deleteProperty(globalThis, 'localStorage');
  }
  expect(listeners.size).toBe(0);
});
