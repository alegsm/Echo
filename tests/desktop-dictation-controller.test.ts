// @ts-nocheck
import { expect, test } from 'bun:test';
import { createDictationController } from '../src/lib/runtime/desktop/dictation-controller';

test('captured artifact is transcribed natively and applied without audio IPC', async () => {
  const harness = createHarness();
  const result = await harness.controller.stop();

  expect(harness.proofs).toEqual([{
    artifactId: 'artifact-a', intentId: 'capture-intent', sessionId: 'session-a'
  }]);
  expect(JSON.stringify(harness.proofs)).not.toContain('audio');
  expect(harness.applied).toEqual([7]);
  expect(harness.transcripts).toEqual(['transcript-capture-intent']);
  expect(harness.completionIdentities).toEqual(['capture-intent']);
  expect(result?.state).toBe('done');
});

test('dispose cancels active native jobs and suppresses late transcript publication', async () => {
  const pending = deferred();
  const harness = createHarness({ transcribe: () => pending.promise });
  const stopping = harness.controller.stop();
  await waitFor(() => harness.proofs.length === 1);
  const disposing = harness.controller.dispose();
  pending.resolve(completion('capture-intent', 9));
  await Promise.all([stopping, disposing]);

  expect(harness.cancelledJobs).toEqual([['session-a', 'capture-intent']]);
  expect(harness.transcripts).toEqual([]);
  expect(harness.applied).toEqual([]);
  expect(harness.delivered).toEqual([]);
});

test('canceling a newer capture does not cancel an older transcription job', async () => {
  const pending = deferred();
  const harness = createHarness({ transcribe: () => pending.promise });
  const stopping = harness.controller.stop();
  await waitFor(() => harness.proofs.length === 1);
  await harness.controller.start();
  await harness.controller.cancel();
  expect(harness.cancelledJobs).toEqual([]);
  pending.resolve(completion('capture-intent', 10));
  await stopping;
  expect(harness.transcripts).toEqual(['transcript-capture-intent']);
});

test('history failure still publishes the completed transcript and surfaces persistence error', async () => {
  const persistenceErrors = [];
  const harness = createHarness({
    completion: { persistence: { errorCode: 'history_write_failed', saved: false } },
    onPersistenceError: () => persistenceErrors.push('history_write_failed')
  });
  const result = await harness.controller.stop();
  expect(result?.lastTranscript?.id).toBe('transcript-capture-intent');
  expect(harness.transcripts).toEqual(['transcript-capture-intent']);
  expect(persistenceErrors).toEqual(['history_write_failed']);
});

function createHarness(overrides = {}) {
  const proofs = [];
  const applied = [];
  const transcripts = [];
  const completionIdentities = [];
  const cancelledJobs = [];
  const delivered = [];
  let deliveryDisposed = false;
  let transcribeCalls = 0;
  const state = {
    apply(envelope) { applied.push(envelope.sequence); },
    async refresh() {},
    getSnapshot() { return echoSnapshot('idle'); }
  };
  const transcription = {
    async transcribe(proof) {
      proofs.push(proof);
      transcribeCalls += 1;
      return overrides.transcribe
        ? overrides.transcribe(proof, transcribeCalls)
        : { ...completion(proof.intentId, 7), ...overrides.completion };
    },
    async cancel(sessionId, intentId) { cancelledJobs.push([sessionId, intentId]); }
  };
  const controller = createDictationController({
    capture: {
      async press() { return { status: 'recording', session: {} }; },
      async release() {
        return { status: 'captured', artifact: artifact(), intentId: 'capture-intent' };
      },
      async cancel() {},
      async dispose() {},
      isHeld() { return false; },
      ...overrides.capture
    },
    delivery: {
      enqueue: (request) => request.then((value) => {
        if (!deliveryDisposed) delivered.push(value.transcript.id);
        return value;
      }),
      dispose() { deliveryDisposed = true; }
    },
    onPersistenceError: overrides.onPersistenceError ?? (() => undefined),
    onTranscript: (transcript, dictationId) => {
      transcripts.push(transcript.id);
      completionIdentities.push(dictationId);
    },
    state,
    transcription
  });
  return { applied, cancelledJobs, completionIdentities, controller, delivered, proofs, transcripts };
}

function completion(intentId, sequence) {
  const transcript = transcriptRecord(`transcript-${intentId}`);
  return {
    delivery: { pasteAfterTranscription: true, restoreClipboard: true },
    jobSequence: sequence,
    persistence: { saved: true },
    snapshot: { sequence, snapshot: { ...echoSnapshot('done'), lastTranscript: transcript } },
    transcript
  };
}

function artifact() {
  return { artifactId: 'artifact-a', byteSize: 1024, channelCount: 1, durationMs: 500, mimeType: 'audio/wav', purpose: 'push_to_talk', sampleRate: 48_000, sessionId: 'session-a' };
}

function transcriptRecord(id) {
  return { id, text: 'hello', createdAt: new Date().toISOString(), durationMs: 500, providerId: 'groq-whisper-turbo', timings: { recordingMs: 500, blobReadyMs: 0, uploadMs: 0, stopToTranscriptMs: 10 }, audio: { mimeType: 'audio/wav', byteSize: 1024 }, insertionMode: 'copied', source: 'desktop' };
}

function echoSnapshot(state) {
  return { activeProviderId: 'groq-whisper-turbo', activeShortcut: 'Fn', mode: 'desktop', state };
}

function deferred() {
  let resolve;
  const promise = new Promise((resolveValue) => { resolve = resolveValue; });
  return { promise, resolve };
}

async function waitFor(predicate) {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    if (predicate()) return;
    await Promise.resolve();
  }
  throw new Error('Condition was not met');
}

test('delivery and transcript publication wait for the one final native result', async () => {
  const pending = deferred();
  const harness = createHarness({ transcribe: () => pending.promise });
  const stopping = harness.controller.stop();
  await waitFor(() => harness.proofs.length === 1);
  expect(harness.transcripts).toEqual([]);
  expect(harness.delivered).toEqual([]);
  expect(harness.applied).toEqual([]);
  pending.resolve(completion('capture-intent', 11));
  await stopping;
  expect(harness.transcripts).toEqual(['transcript-capture-intent']);
  expect(harness.delivered).toEqual(['transcript-capture-intent']);
  expect(harness.applied).toEqual([11]);
});

test('cancel during capture prevents final STT and delivery', async () => {
  let cancelled = false;
  const harness = createHarness({ capture: {
    async cancel() { cancelled = true; },
    async release() { return cancelled ? { status: 'cancelled' } : { status: 'captured', artifact: artifact(), intentId: 'capture-intent' }; }
  } });
  await harness.controller.start();
  await harness.controller.cancel();
  await harness.controller.stop();
  expect(harness.proofs).toEqual([]);
  expect(harness.transcripts).toEqual([]);
  expect(harness.delivered).toEqual([]);
});
