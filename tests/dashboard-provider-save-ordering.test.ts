import { expect, test } from 'bun:test';
import { defaultSettings, type EchoSettings } from '../src/lib/core/settings';
import { createDesktopRuntime } from '../src/lib/runtime/desktop/desktop-runtime';
import type { NativeCaptureBoundary } from '../src/lib/runtime/desktop/native-capture-client';
import { createDashboardController, initialDashboardState } from '../src/lib/ui/dashboard/dashboard-controller';
import { createProviderController, type ProviderState } from '../src/lib/ui/dashboard/panels/provider-controller';

const id = defaultSettings.providerId;
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}
async function drain() { for (let i = 0; i < 100; i++) await Promise.resolve(); }

// Keep: real controllers and runtime, with only native IO replaced. No OS store is opened.
function harness() {
  let settings: EchoSettings = structuredClone(defaultSettings);
  let revision = 0, saves = 0, probes = 0, conflicts = 0;
  const listeners = new Map<string, (event: { payload: unknown }) => void>();
  const initial = deferred<void>();
  let holdInitial = false;
  let probeError: string | undefined;
  let settingsError: string | undefined;
  let updateAttempts = 0;
  let barrier: { command: string; phase: 'before' | 'after'; remaining: number; entered: ReturnType<typeof deferred<void>>; release: ReturnType<typeof deferred<void>> } | undefined;
  async function pause(command: string, phase: 'before' | 'after') {
    const gate = barrier;
    if (!gate || gate.command !== command || gate.phase !== phase || --gate.remaining !== 0) return;
    barrier = undefined;
    gate.entered.resolve();
    await gate.release.promise;
  }
  function emit() { listeners.get('echo-settings-committed')?.({ payload: { revision, changedFields: ['providerKeySaved'] } }); }
  async function invoke(command: string, args?: Record<string, unknown>): Promise<unknown> {
    if (command === 'get_persisted_settings') { if (holdInitial) await initial.promise; return { revision, settings: structuredClone(settings) }; }
    if (command === 'get_desktop_runtime_snapshot') return { sequence: 0, snapshot: { mode: 'desktop', state: 'idle', activeProviderId: id, activeShortcut: 'Fn' } };
    if (command === 'provider_api_key_presence') return settings.providerKeySaved;
    if (command === 'set_provider_api_key' || command === 'delete_provider_api_key') {
      if (command === 'set_provider_api_key') saves++;
      settings.providerKeySaved = { ...settings.providerKeySaved, [id]: command === 'set_provider_api_key' };
      revision++;
      return;
    }
    if (command === 'update_persisted_settings') {
      updateAttempts++;
      if (settingsError) throw settingsError;
      if (args?.expectedRevision !== revision) { conflicts++; throw `settings revision conflict: expected ${args?.expectedRevision}, actual ${revision}`; }
      const previous = { revision, settings: structuredClone(settings) };
      const patch = args.patch as Partial<EchoSettings>;
      settings = { ...settings, ...patch, providerKeyStatuses: { ...settings.providerKeyStatuses, ...patch.providerKeyStatuses } };
      revision++;
      return { previous, current: { revision, settings: structuredClone(settings) }, changedFields: Object.keys(patch), cleanupStatus: 'complete' };
    }
    if (command === 'probe_provider') { probes++; if (probeError) throw { code: probeError, message: 'private native detail' }; return { ok: true, providerId: id, providerModel: 'whisper-large-v3-turbo', timings: { backendMs: 1, providerMs: 1 } }; }
    return undefined;
  }
  const boundary: NativeCaptureBoundary = {
    invoke: async <T>(command: string, args?: Record<string, unknown>) => {
      await pause(command, 'before');
      try { return await invoke(command, args) as T; }
      finally { await pause(command, 'after'); }
    },
    listen: async <T>(name: string, listener: (event: { payload: T }) => void) => {
      listeners.set(name, listener as (event: { payload: unknown }) => void);
      return () => { listeners.delete(name); };
    }
  };
  const runtime = createDesktopRuntime({ boundary, hotkeyOwner: false, syncDiagnostics: async () => {},
    hotkeyBoundary: { listenFn: async () => () => {}, register: async () => {}, unregister: async () => {}, setFnCaptureEnabled: async () => false } });
  const state = initialDashboardState({ tab: 'settings' });
  const dashboard = createDashboardController(state, { tick: async () => {}, focusPermissions() {} });
  const providerState: ProviderState = { operation: { busy: false, checking: null }, messages: {}, resultTones: {}, removalProvider: null };
  const provider = createProviderController(dashboard.context, providerState, { tick: async () => {}, dialog: () => null, activeElement: () => null, focusKey() {} });
  return { runtime, state, dashboard, provider, providerState, emit,
    counts: () => ({ saves, probes, conflicts }),
    updateAttempts: () => updateAttempts,
    rejectSettings(error: string) { settingsError = error; },
    holdInitial() { holdInitial = true; }, releaseInitial() { holdInitial = false; initial.resolve(); },
    externalWrite() { settings.language = 'de'; revision++; },
    rejectProbe(code: string) { probeError = code; },
    hold(command: string, phase: 'before' | 'after', occurrence = 1) {
      const entered = deferred<void>(), release = deferred<void>();
      barrier = { command, phase, remaining: occurrence, entered, release };
      return { entered: entered.promise, release: () => release.resolve() };
    } };
}

test('first acknowledged save completes even before its settings event; second check needs no repair', async () => {
  const h = harness();
  const unmount = h.dashboard.mount(() => h.runtime);
  try {
    await h.runtime.getSettings(); await drain();
    h.dashboard.context.setApiKeyDraft(id, '<redacted>');
    await h.provider.verifyProvider(id);
    await h.provider.confirmSave();
    const first = { ...h.counts(), tone: h.providerState.resultTones[id] };
    await h.provider.verifyProvider(id);
    const second = { ...h.counts(), tone: h.providerState.resultTones[id] };
    console.info('save ordering evidence', { first, second });
    expect(first).toEqual({ saves: 1, probes: 1, conflicts: 0, tone: 'success' });
    expect(second).toEqual({ saves: 1, probes: 2, conflicts: 0, tone: 'success' });
  } finally { unmount(); await h.runtime.dispose(); }
});

test('late initial settings response preserves a draft entered while loading', async () => {
  const h = harness(); h.holdInitial();
  const unmount = h.dashboard.mount(() => h.runtime);
  try {
    h.dashboard.context.setApiKeyDraft(id, '<redacted>');
    h.releaseInitial();
    await h.runtime.getSettings(); await drain();
    expect(h.dashboard.context.apiKeyDrafts()[id]).toBe('<redacted>');
  } finally { unmount(); await h.runtime.dispose(); }
});

// Keep: native response gates exercise settings events at each success/error await.
for (const outcome of ['success', 'provider_credential_rejected', 'provider_timeout'] as const) {
  const stages: [string, number][] = [
    ['set_provider_api_key', 1], ['get_persisted_settings', 1],
    ['update_persisted_settings', 1], ['probe_provider', 1],
    ...(outcome === 'provider_timeout' ? [] : [['update_persisted_settings', 2] as [string, number]]),
    ['provider_api_key_presence', 1]
  ];
  for (const [command, occurrence] of stages) for (const phase of ['before', 'after'] as const) {
    for (const arrival of ['before', 'during', 'after'] as const) {
      test(`${outcome}: settings event ${arrival} ${command} #${occurrence} ${phase} response gate`, async () => {
        const h = harness(); const unmount = h.dashboard.mount(() => h.runtime);
        try {
          await h.runtime.getSettings(); await drain();
          if (outcome !== 'success') h.rejectProbe(outcome);
          h.dashboard.context.setApiKeyDraft(id, '<redacted>');
          await h.provider.verifyProvider(id);
          const gate = h.hold(command, phase, occurrence);
          if (arrival === 'before') h.emit();
          const checking = h.provider.confirmSave();
          await gate.entered;
          if (arrival === 'during') { h.emit(); await drain(); }
          gate.release();
          if (arrival === 'after') { await Promise.resolve(); h.emit(); }
          await checking; await drain();
          expect(h.counts()).toEqual({ saves: 1, probes: 1, conflicts: 0 });
          expect(h.state.settings.providerKeySaved?.[id]).toBe(true);
          expect(h.state.settings.providerKeyStatuses?.[id]).toBe(outcome === 'success' ? 'valid' : outcome === 'provider_timeout' ? 'unchecked' : 'invalid');
          expect(h.providerState.resultTones[id]).toBe(outcome === 'success' ? 'success' : 'error');
          expect(h.providerState.messages[id]).not.toContain('private');
          expect(h.providerState.operation.busy).toBe(false);
        } finally { unmount(); await h.runtime.dispose(); }
      });
    }
  }
}

test('a newer draft survives the old save and prevents its probe through the real runtime', async () => {
  const h = harness(); const unmount = h.dashboard.mount(() => h.runtime);
  try {
    await h.runtime.getSettings(); await drain();
    h.dashboard.context.setApiKeyDraft(id, '<redacted>');
    await h.provider.verifyProvider(id);
    const gate = h.hold('set_provider_api_key', 'after');
    const checking = h.provider.confirmSave(); await gate.entered;
    h.dashboard.context.setApiKeyDraft(id, 'newer synthetic draft');
    h.emit(); await drain(); gate.release(); await checking;
    expect(h.dashboard.context.apiKeyDrafts()[id]).toBe('newer synthetic draft');
    expect(h.counts()).toEqual({ saves: 1, probes: 0, conflicts: 0 });
    expect(h.providerState.resultTones[id]).toBeUndefined();
  } finally { unmount(); await h.runtime.dispose(); }
});

test('confirmed removal refreshes its revision before status writes and preserves a newer draft', async () => {
  const h = harness(); const unmount = h.dashboard.mount(() => h.runtime);
  try {
    await h.runtime.getSettings(); await drain();
    h.dashboard.context.setApiKeyDraft(id, '<redacted>');
    await h.provider.verifyProvider(id); await h.provider.confirmSave();
    h.provider.requestRemoveKey(id);
    const gate = h.hold('delete_provider_api_key', 'after');
    const removing = h.provider.removeProviderKey(); await gate.entered;
    h.dashboard.context.setApiKeyDraft(id, 'newer synthetic draft');
    gate.release(); await removing;
    expect(h.state.settings.providerKeySaved?.[id]).toBe(false);
    expect(h.state.settings.providerKeyStatuses?.[id]).toBe('unchecked');
    expect(h.state.providerKeyPresence[id]).toBe(false);
    expect(h.dashboard.context.apiKeyDrafts()[id]).toBe('newer synthetic draft');
    expect(h.counts().conflicts).toBe(0);
  } finally { unmount(); await h.runtime.dispose(); }
});

// Keep: another window can commit after the credential reload and before the status reset.
test('a status conflict after the post-save reload retries without repeating the save or probe', async () => {
  const h = harness(); const unmount = h.dashboard.mount(() => h.runtime);
  try {
    await h.runtime.getSettings(); await drain();
    h.dashboard.context.setApiKeyDraft(id, '<redacted>');
    await h.provider.verifyProvider(id);
    const gate = h.hold('update_persisted_settings', 'before');
    const checking = h.provider.confirmSave(); await gate.entered;
    h.externalWrite();
    gate.release(); await checking;
    expect(h.counts()).toEqual({ saves: 1, probes: 1, conflicts: 1 });
    expect(h.state.settings.language).toBe('de');
    expect(h.state.settings.providerKeyStatuses?.[id]).toBe('valid');
    expect(h.providerState.resultTones[id]).toBe('success');
  } finally { unmount(); await h.runtime.dispose(); }
});

test('a late initial settings response preserves the hotkey draft being edited', async () => {
  const h = harness(); h.holdInitial();
  const unmount = h.dashboard.mount(() => h.runtime);
  try {
    h.dashboard.context.setHotkeyDraft('Control+K');
    h.releaseInitial();
    await h.runtime.getSettings(); await drain();
    expect(h.dashboard.context.hotkeyDraft()).toBe('Control+K');
  } finally { unmount(); await h.runtime.dispose(); }
});

test.each([{ language: 'fr' }, { language: 'fr', providerKeyStatuses: { [id]: 'unchecked' } }] satisfies Partial<EchoSettings>[])('an unrelated or mixed settings write still reports a revision conflict: %j', async (patch) => {
  const h = harness();
  try {
    await h.runtime.getSettings();
    h.externalWrite();
    await expect(h.runtime.updateSettings(patch)).rejects.toBe('settings revision conflict: expected 0, actual 1');
    expect((await h.runtime.getSettings()).language).toBe('de');
    expect(h.counts().conflicts).toBe(1);
  } finally { await h.runtime.dispose(); }
});

test('a second status conflict is returned without another retry', async () => {
  const h = harness();
  try {
    await h.runtime.getSettings();
    const first = h.hold('update_persisted_settings', 'before');
    const updating = h.runtime.updateSettings({ providerKeyStatuses: { [id]: 'unchecked' } });
    const rejected = updating.catch((error: unknown) => error);
    await first.entered; h.externalWrite();
    const second = h.hold('update_persisted_settings', 'before');
    first.release();
    await second.entered; h.externalWrite(); second.release();
    expect(await rejected).toBe('settings revision conflict: expected 1, actual 2');
    expect(h.counts().conflicts).toBe(2);
  } finally { await h.runtime.dispose(); }
});

// Keep: a reload without a newer revision does not justify retrying a failed status write.
test('a status failure with no newer committed revision is returned without retry', async () => {
  const h = harness();
  try {
    await h.runtime.getSettings();
    h.rejectSettings('settings storage unavailable');
    await expect(h.runtime.updateSettings({ providerKeyStatuses: { [id]: 'unchecked' } }))
      .rejects.toBe('settings storage unavailable');
    expect(h.updateAttempts()).toBe(1);
  } finally { await h.runtime.dispose(); }
});
