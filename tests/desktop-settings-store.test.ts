// @ts-nocheck
import { expect, test } from 'bun:test';
import { createNativeSettingsStore } from '../src/lib/runtime/desktop/native-settings-store';

test('settings store subscribes before read and rejects a stale startup snapshot', async () => {
  const firstRead = deferred();
  const reads = [firstRead.promise, Promise.resolve(envelope(3, 'es'))];
  const harness = createHarness({ reads });
  const store = createNativeSettingsStore(harness.client);
  const seen = [];
  store.subscribe((settings) => seen.push(settings.language));
  const starting = store.start();
  await waitFor(() => harness.listener !== undefined);
  await waitFor(() => harness.readIndex === 1);
  harness.listener({ revision: 3, changedFields: ['language'] });
  await waitFor(() => seen[0] === 'es');
  firstRead.resolve(envelope(2, 'en'));
  await starting;

  expect(seen).toEqual(['es']);
  expect(store.get()?.language).toBe('es');
});

test('revision conflict reloads committed settings before returning the error', async () => {
  const conflict = 'settings revision conflict: expected 4, actual 5';
  const harness = createHarness({
    reads: [Promise.resolve(envelope(4, 'en')), Promise.resolve(envelope(5, 'fr'))],
    update: async () => { throw conflict; }
  });
  const store = createNativeSettingsStore(harness.client);
  await store.start();
  await expect(store.update({ language: 'de' })).rejects.toBe(conflict);
  expect(store.get()?.language).toBe('fr');
});

function createHarness(options) {
  const harness = { listener: undefined, readIndex: 0, unlistenCalls: 0 };
  harness.client = {
    async listenSettings(listener) {
      harness.listener = listener;
      return () => { harness.unlistenCalls += 1; };
    },
    async getSettings() {
      return options.reads[harness.readIndex++];
    },
    updateSettings: options.update ?? (async (_revision, patch) => ({
      current: envelope(2, patch.language), previous: envelope(1, 'en'), changedFields: ['language']
    }))
  };
  return harness;
}

function envelope(revision, language) {
  return {
    revision,
    settings: {
      pushToTalkShortcut: 'Fn', providerId: 'groq-whisper-turbo', language,
      pasteAfterTranscription: true, restoreClipboard: true,
      debugLoggingEnabled: false
    }
  };
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
