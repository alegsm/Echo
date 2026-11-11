import { expect, test } from 'bun:test';
import { createProviderController, type ProviderState } from '../src/lib/ui/dashboard/panels/provider-controller';
import { defaultSettings, providerOptions } from '../src/lib/core/settings';
import type { EchoSettings, ProviderId } from '../src/lib/core/settings';
import type { DesktopPermissions, EchoSnapshot, ProviderKeySearchOutcome } from '../src/lib/core/types';

// Keep: directly imported provider operations with a simulated credential boundary. Live Keychain and browser gates belong to the host harness.
async function providerHandlers(present = true, selectedId: ProviderId = providerOptions[0].id) {
  const id = selectedId;
  let settings: EchoSettings = { ...defaultSettings };
  const drafts = Object.fromEntries(providerOptions.map((p) => [p.id, ''])) as Record<ProviderId, string>;
  const calls: string[] = [];
  const resolvedStatuses: Array<string | undefined> = [];
  const gates = new Map<string, { entered: () => void; wait: Promise<void> }>();
  const failures = new Set<string>();
  async function boundary(name: string) {
    const gate = gates.get(name); gate?.entered(); await gate?.wait;
    if (failures.has(name)) throw new Error('private native detail');
  }
  let rejection: unknown, saves = 0, stored = present;
  let outcome: ProviderKeySearchOutcome = 'not_found', snapshotState: EchoSnapshot['state'] = 'idle';
  let platform: DesktopPermissions['platform'] = 'macos', offline = false;
  const ctx = {
    settings: () => settings, snapshot: () => ({ state: snapshotState }),
    permissions: () => ({ platform }) as DesktopPermissions,
    apiKeyDrafts: () => drafts,
    providerKeyPresence: () => Object.fromEntries(providerOptions.map((p) => [p.id, p.id === id && present])) as Record<ProviderId, boolean>, setActionError() {},
    setApiKeyDraft(provider: ProviderId, value: string) { drafts[provider] = value; },
    async updateSettings(patch: Partial<EchoSettings>) {
      if (patch.providerKeyStatuses?.[id] === 'unchecked') { calls.push('reset'); await boundary('reset'); }
      settings = { ...settings, ...patch, providerKeyStatuses: { ...settings.providerKeyStatuses, ...patch.providerKeyStatuses } }; return settings;
    },
    refreshProviderKeyPresence: async () => { calls.push("refresh"); if (failures.has('refresh')) throw new Error('private native detail'); present = stored; },
    runtime: () => offline ? undefined : ({ async setProviderApiKey(provider: ProviderId) { if (provider !== id) throw new Error('wrong provider'); saves++; calls.push('store'); await boundary('store'); stored = true; },
      async deleteProviderApiKey(provider: ProviderId) { calls.push(`delete:${provider}`); if (rejection) throw rejection; stored = false; },
      async findProviderApiKey(provider: ProviderId) { calls.push(`find:${provider}`); await boundary('find'); if (rejection) throw rejection; if (outcome === 'found') stored = true; return outcome; },
      async probeProviderKey(request: { providerId: ProviderId }) { if (request.providerId !== id) throw new Error('wrong provider'); calls.push('probe'); resolvedStatuses.push(settings.providerKeyStatuses?.[id]); await boundary('probe'); if (rejection) throw rejection; return { ok: true as const, providerId: id, providerModel: providerOptions.find((p) => p.id === id)!.model, timings: { backendMs: 10 } }; } })
  };
  function mount() {
    const state: ProviderState = { operation: { busy: false, checking: null }, messages: {}, resultTones: {}, removalProvider: null };
    let dialog: { showModal(): void; close(): void } | undefined;
    const controller = createProviderController(ctx, state, {
      tick: async () => {}, dialog: () => dialog, activeElement: () => null,
      focusKey() { calls.push('focus:provider-key'); },
      focusCheck() { calls.push('focus:check-key'); }, focusFind() { calls.push('focus:find-key'); }
    });
    const actions = { ...controller,
      async checkAndSave(id: ProviderId) { await controller.verifyProvider(id); if (state.pendingSave) await controller.confirmSave(); },
      message: (id: ProviderId) => state.messages[id], tone: (id: ProviderId) => state.resultTones[id],
      pending: () => state.operation.checking, finding: () => state.operation.finding,
      setRemovalDialog(value: typeof dialog) { dialog = value; }
    };
    return { actions, destroy: controller.dispose };
  }
  return { ...mount(), remount: mount, calls, resolvedStatuses, ctx, id, saves: () => saves, reject: (error: unknown) => { rejection = error; },
    search(value: ProviderKeySearchOutcome) { outcome = value; },
    offline(value: boolean) { offline = value; },
    platform(value: DesktopPermissions['platform']) { platform = value; },
    recording(value: EchoSnapshot['state']) { snapshotState = value; },
    fail: (name: string, enabled = true) => enabled ? failures.add(name) : failures.delete(name),
    hold(name: string) {
      let release!: () => void, entered!: () => void;
      const started = new Promise<void>((resolve) => { entered = resolve; });
      const wait = new Promise<void>((resolve) => { release = resolve; });
      gates.set(name, { entered, wait });
      return { started, release };
    } };
}

test('Check key probes an existing Keychain credential without writing an empty draft', async () => {
  const h = await providerHandlers();
  await h.actions.checkAndSave(h.id);
  expect(h.saves()).toBe(0);
  expect(h.calls).toContain('probe');
});

test('a new draft is stored and cleared before verification', async () => {
  const h = await providerHandlers();
  h.ctx.setApiKeyDraft(h.id, 'synthetic-input');
  await h.actions.checkAndSave(h.id);
  expect(h.calls.slice(0, 3)).toEqual(['store', 'reset', 'probe']);
  expect(h.ctx.apiKeyDrafts()[h.id]).toBe('');
});

// Keep: only credential errors warn; transient failures leave existing status unchanged.
test.each([
  { error: { code: 'provider_credential_rejected' }, status: 'invalid' },
  { error: { code: 'missing_provider_key' }, status: 'invalid' },
  { error: { code: 'credential_unavailable' }, status: 'invalid' },
  { error: new Error('private native detail'), status: undefined }
])('failed checks warn, credential errors persist invalid, and replacement or success clears it: $status', async ({ error, status }) => {
  const h = await providerHandlers();
  h.reject(error);
  await h.actions.checkAndSave(h.id);
  expect(h.actions.message(h.id)).toBe(status === 'invalid' ? 'Key unavailable or rejected. Update it and try again.' : 'Key check failed. Try again.');
  expect(h.ctx.settings().providerKeyStatuses?.[h.id]).toBe(status);
  expect(h.actions.warning(h.id)).toBe(status === 'invalid');
  expect(h.actions.pending()).toBeNull();
  h.reject({ code: 'provider_timeout' });
  await h.actions.checkAndSave(h.id);
  expect(h.ctx.settings().providerKeyStatuses?.[h.id]).toBe(status);
  h.ctx.setApiKeyDraft(h.id, 'synthetic-replacement');
  await h.actions.checkAndSave(h.id);
  expect(h.ctx.settings().providerKeyStatuses?.[h.id]).toBe('unchecked');
  h.reject(undefined);
  await h.actions.checkAndSave(h.id);
  expect(h.ctx.settings().providerKeyStatuses?.[h.id]).toBe('valid');
  expect(h.actions.warning(h.id)).toBe(false);
});

// Keep: every provider uses the existing native selection, storage and check path.
for (const provider of providerOptions) test(`${provider.label} selects, configures and checks its own credential`, async () => {
  const h = await providerHandlers(false, provider.id);
  await h.actions.selectProvider(provider.id);
  expect(h.ctx.settings().providerId).toBe(provider.id);
  h.ctx.setApiKeyDraft(provider.id, 'synthetic-input');
  await h.actions.checkAndSave(provider.id);
  expect(h.calls.filter((c) => ['store', 'reset', 'probe'].includes(c))).toEqual(['store', 'reset', 'probe']);
  expect(h.ctx.settings().providerKeyStatuses?.[provider.id]).toBe('valid');
});

// Keep: confirmed deletion crosses the credential boundary and reconciles displayed presence.
test('Remove key requires confirmation, resets status, refreshes presence and returns focus', async () => {
  const h = await providerHandlers();
  h.actions.setRemovalDialog({ showModal() { h.calls.push('confirm'); }, close() { h.calls.push('close'); void h.actions.closeRemovalDialog(); } });
  await h.actions.removeProviderKey();
  expect(h.calls).toEqual([]);
  await h.actions.checkAndSave(h.id);
  h.calls.length = 0;
  h.actions.requestRemoveKey(h.id);
  expect(h.calls).toEqual(['confirm']);
  expect(h.ctx.providerKeyPresence()[h.id]).toBe(true);
  await h.actions.removeProviderKey();
  expect(h.calls).toEqual([`confirm`, `delete:${h.id}`, 'reset', 'refresh', 'close', 'focus:provider-key']);
  expect(h.ctx.providerKeyPresence()[h.id]).toBe(false);
  expect(h.ctx.settings().providerKeyStatuses?.[h.id]).toBe('unchecked');
  h.calls.length = 0;
  h.actions.requestRemoveKey(h.id);
  await h.actions.removeProviderKey();
  expect(h.calls).toEqual([]);
});

test('Remove key does nothing without presence', async () => {
  const h = await providerHandlers(false);
  h.actions.requestRemoveKey(h.id);
  await h.actions.removeProviderKey();
  expect(h.calls).toEqual([]);
});

test('a failed removal keeps presence and reports a fixed retryable error', async () => {
  const h = await providerHandlers();
  h.actions.setRemovalDialog({ showModal() {}, close() {} });
  h.reject(new Error('private native detail'));
  h.actions.requestRemoveKey(h.id);
  await h.actions.removeProviderKey();
  expect(h.ctx.providerKeyPresence()[h.id]).toBe(true);
  expect(h.actions.message(h.id)).toBe('Key could not be removed. Try again.');
  expect(h.actions.tone(h.id)).toBe('error');
  expect(h.actions.warning(h.id)).toBe(false);
});

test('check feedback includes the success time or a sanitized retryable failure with its tone', async () => {
  const h = await providerHandlers();
  await h.actions.checkAndSave(h.id);
  expect(h.actions.message(h.id)).toMatch(/^Key works, checked at .+/);
  expect(h.actions.tone(h.id)).toBe('success');
  h.reject(new Error('private native detail'));
  await h.actions.checkAndSave(h.id);
  expect(h.actions.message(h.id)).toBe('Key check failed. Try again.');
  expect(h.actions.tone(h.id)).toBe('error');
});

test('dialog Tab and Shift+Tab wrap on every press and leave Escape available', async () => {
  const h = await providerHandlers();
  let focused = 0, prevented = 0;
  const button = { focus() { focused++; } };
  for (const shiftKey of [false, false, true, true]) {
    h.actions.trapDialogFocus({ key: 'Tab', shiftKey, preventDefault() { prevented++; },
      currentTarget: { querySelectorAll() { return [button]; } } } as unknown as KeyboardEvent);
  }
  h.actions.trapDialogFocus({ key: 'Escape', preventDefault() { throw new Error('Escape blocked'); } } as unknown as KeyboardEvent);
  expect(focused).toBe(4);
  expect(prevented).toBe(4);
});

// Keep: deferred native boundaries expose ordering, stale results and serialized retries.
test('replacement status resets before probing and pending checks are serialized', async () => {
  const h = await providerHandlers();
  h.ctx.setApiKeyDraft(h.id, 'synthetic-input');
  const gate = h.hold('reset');
  const checking = h.actions.checkAndSave(h.id);
  await gate.started;
  expect(h.calls).not.toContain('probe');
  expect(h.actions.pending()).toBe(h.id);
  expect(h.actions.message(h.id)).toBe('Checking key with Groq');
  await h.actions.checkAndSave(h.id);
  expect(h.calls.filter((call) => call === 'reset')).toHaveLength(1);
  gate.release(); await checking;
  expect(h.resolvedStatuses).toEqual(['unchecked']);
  expect(h.actions.pending()).toBeNull();
});

test.each(['provider', 'draft'])('a changed %s discards both success and failure without rewriting selection', async (change) => {
  for (const failed of [false, true]) {
    const h = await providerHandlers();
    const gate = h.hold('probe');
    const checking = h.actions.checkAndSave(h.id);
    await gate.started;
    if (change === 'provider') await h.ctx.updateSettings({ providerId: providerOptions[1].id });
    if (change === 'draft') h.ctx.setApiKeyDraft(h.id, 'synthetic-newer-input');
    const before = structuredClone(h.ctx.settings());
    if (failed) h.reject({ code: 'provider_credential_rejected', message: 'private native detail' });
    gate.release();
    await checking;
    expect(h.ctx.settings()).toEqual(before);
    expect(h.actions.tone(h.id)).toBeUndefined();
    expect(h.actions.message(h.id)).toBe('');
    expect(h.actions.pending()).toBeNull();
  }
});

test('a newer draft survives storage of the old draft and stops its probe', async () => {
  const h = await providerHandlers();
  h.ctx.setApiKeyDraft(h.id, 'synthetic-first-input');
  const gate = h.hold('store');
  const checking = h.actions.checkAndSave(h.id);
  await gate.started;
  h.ctx.setApiKeyDraft(h.id, 'synthetic-newer-input');
  gate.release();
  await checking;
  expect(h.ctx.apiKeyDrafts()[h.id]).toBe('synthetic-newer-input');
  expect(h.calls).not.toContain('probe');
  expect(h.actions.tone(h.id)).toBeUndefined();
});

test('a stored key stays configured when its probe fails', async () => {
  const h = await providerHandlers(false);
  h.ctx.setApiKeyDraft(h.id, 'synthetic-input');
  h.reject({ code: 'provider_credential_rejected' });
  await h.actions.checkAndSave(h.id);
  expect(h.ctx.providerKeyPresence()[h.id]).toBe(true);
  expect(h.actions.warning(h.id)).toBe(true);
});

test.each(['reset', 'store'])('a failed %s commit stops the probe and allows retry', async (boundary) => {
  const h = await providerHandlers();
  h.ctx.setApiKeyDraft(h.id, 'synthetic-input');
  h.fail(boundary);
  await h.actions.checkAndSave(h.id);
  expect(h.calls).not.toContain('probe');
  if (boundary === 'store') expect(h.ctx.apiKeyDrafts()[h.id]).toBe('synthetic-input');
  expect(h.actions.message(h.id)).toBe('Key check failed. Try again.');
  expect(h.actions.pending()).toBeNull();
  h.fail(boundary, false);
  await h.actions.checkAndSave(h.id);
  expect(h.actions.tone(h.id)).toBe('success');
});

test('a Settings remount cannot start another check until the old native probe settles', async () => {
  const h = await providerHandlers();
  const gate = h.hold('probe');
  const first = h.actions.checkAndSave(h.id);
  await gate.started;
  h.destroy();
  const next = h.remount();
  expect(next.actions.pending()).toBe(h.id);
  const duplicate = next.actions.checkAndSave(h.id);
  // Both attempts have reached their first async boundary before inspecting calls.
  await Promise.resolve();
  await Promise.resolve();
  expect(h.calls.filter((call) => call === 'probe')).toHaveLength(1);
  gate.release();
  await Promise.all([first, duplicate]);
  await next.actions.checkAndSave(h.id);
  expect(next.actions.tone(h.id)).toBe('success');
});

// Keep: remounting changes local message ownership, not the credential operation.
test.each([false, true])('a check settling after Settings remount persists its status, failed: %s', async (failed) => {
  const h = await providerHandlers();
  await h.ctx.updateSettings({ providerKeyStatuses: { [h.id]: 'invalid' } });
  const gate = h.hold('probe');
  const checking = h.actions.checkAndSave(h.id);
  await gate.started;
  h.destroy();
  const next = h.remount();
  if (failed) {
    await h.ctx.updateSettings({ providerKeyStatuses: { [h.id]: 'unchecked' } });
    h.reject({ code: 'provider_credential_rejected' });
  }
  gate.release();
  await checking;
  expect(h.ctx.settings().providerKeyStatuses?.[h.id]).toBe(failed ? 'invalid' : 'valid');
  expect(next.actions.warning(h.id)).toBe(failed);
  expect(h.actions.message(h.id)).toBe('');
  expect(h.actions.tone(h.id)).toBeUndefined();
  expect(next.actions.message(h.id)).toBeUndefined();
  expect(next.actions.pending()).toBeNull();
});

// Keep: configured state comes from saved metadata, independent of unsaved drafts.
test('presence stays configured while editing a replacement', async () => {
  const h = await providerHandlers();
  h.ctx.setApiKeyDraft(h.id, 'synthetic-input');
  expect(h.actions.configured(h.id)).toBe(true);
  const absent = await providerHandlers(false);
  expect(absent.actions.configured(absent.id)).toBe(false);
});

test('a later native transcription success clears a previous local credential warning', async () => {
  const h = await providerHandlers();
  h.reject({ code: 'provider_credential_rejected' });
  await h.actions.checkAndSave(h.id);
  expect(h.actions.warning(h.id)).toBe(true);
  await h.ctx.updateSettings({ providerKeyStatuses: { [h.id]: 'valid' } });
  expect(h.actions.warning(h.id)).toBe(false);
});

// Keep: cancellation and confirmed removal share one focus restoration path.
test('closing Remove key without confirmation restores focus without deleting', async () => {
  const h = await providerHandlers();
  h.actions.setRemovalDialog({ showModal() {}, close() {} });
  h.actions.requestRemoveKey(h.id);
  await h.actions.closeRemovalDialog();
  await h.actions.removeProviderKey();
  expect(h.calls).toEqual(['focus:provider-key']);
});

// Keep: consent is renewed per draft and cancellation preserves the unsaved key.
test('drafts wait for acknowledgement, cancel cleanly and require it again', async () => {
  const h = await providerHandlers(false);
  h.ctx.setApiKeyDraft(h.id, 'synthetic-input');
  await h.actions.verifyProvider(h.id);
  expect(h.calls).toEqual([]);
  expect(h.actions.providerLocked()).toBe(true);
  await h.actions.verifyProvider(h.id);
  await h.actions.cancelSave();
  expect(h.ctx.apiKeyDrafts()[h.id]).toBe('synthetic-input');
  expect(h.ctx.settings().providerKeyStatuses?.[h.id]).toBeUndefined();
  expect(h.calls).toEqual(['focus:provider-key']);
  h.calls.length = 0;
  await h.actions.verifyProvider(h.id);
  await h.actions.confirmSave();
  expect(h.calls.slice(0, 3)).toEqual(['store', 'reset', 'probe']);
  h.calls.length = 0;
  h.ctx.setApiKeyDraft(h.id, 'synthetic-replacement');
  await h.actions.verifyProvider(h.id);
  expect(h.calls).toEqual([]);
});

test.each(['provider', 'draft', 'unmount'])('changed %s invalidates pending acknowledgement', async (change) => {
  const h = await providerHandlers();
  h.ctx.setApiKeyDraft(h.id, 'synthetic-input');
  await h.actions.verifyProvider(h.id);
  if (change === 'provider') await h.ctx.updateSettings({ providerId: providerOptions[1].id });
  if (change === 'draft') h.ctx.setApiKeyDraft(h.id, 'synthetic-newer');
  if (change === 'unmount') h.destroy();
  await h.actions.confirmSave();
  expect(h.calls).not.toContain('store');
  expect(h.calls).not.toContain('probe');
  expect(h.actions.providerLocked()).toBe(false);
});

// Keep: Find saved key crosses the simulated native search; only an explicit request reaches it.
test('Find saved key searches the selected provider, resets a stale warning and moves focus to Check key', async () => {
  const h = await providerHandlers(false);
  await h.ctx.updateSettings({ providerKeyStatuses: { [h.id]: 'invalid' } });
  h.search('found');
  await h.actions.findSavedKey(h.id);
  expect(h.calls).toEqual([`find:${h.id}`, 'reset', 'refresh', 'focus:check-key']);
  expect(h.ctx.settings().providerKeyStatuses?.[h.id]).toBe('unchecked');
  expect(h.actions.warning(h.id)).toBe(false);
  expect(h.actions.configured(h.id)).toBe(true);
  expect(h.actions.message(h.id)).toBe('Found your saved Groq key. Select Check key to test it.');
  expect(h.actions.tone(h.id)).toBe('success');
  expect(h.actions.providerLocked()).toBe(false);
  h.calls.length = 0;
  await h.actions.findSavedKey(h.id);
  expect(h.calls).toEqual([]);
});

for (const platform of ['macos', 'windows'] as const) {
  const store = platform === 'windows' ? 'Credential Manager' : 'Keychain';
  test.each([
    { outcome: 'not_found', message: `No saved Groq key in ${store}. Paste your key and select Check key.`, tone: undefined, calls: [] },
    { outcome: 'invalid', message: `The saved Groq key in ${store} can't be used. Paste your key and select Check key.`, tone: 'error', calls: [] },
    { outcome: 'unavailable', message: platform === 'windows' ? 'Credential Manager could not be read. Try again.'
      : 'Keychain access was denied or unavailable. Select Find saved key again and choose Allow.', tone: 'error', calls: [] },
    { outcome: 'already_saved', message: '', tone: undefined, calls: ['refresh'] },
    { outcome: 'save_failed', message: 'Key found, but Echo could not save that. Try again.', tone: 'error', calls: [] },
    { outcome: 'rejected', message: 'Saved key search failed. Try again.', tone: 'error', calls: ['refresh'] }
  ] as const)(`${platform} search $outcome reports its copy and tone and keeps focus on Find saved key`, async ({ outcome, message, tone, calls }) => {
    const h = await providerHandlers(false);
    h.platform(platform);
    await h.ctx.updateSettings({ providerKeyStatuses: { [h.id]: 'invalid' } });
    h.calls.length = 0;
    if (outcome === 'rejected') h.reject(new Error('private native detail'));
    else h.search(outcome);
    await h.actions.findSavedKey(h.id);
    expect(h.calls).toEqual([`find:${h.id}`, ...calls, 'focus:find-key']);
    expect(h.actions.message(h.id)).toBe(message);
    expect(h.actions.tone(h.id)).toBe(tone);
    expect(h.ctx.settings().providerKeyStatuses?.[h.id]).toBe('invalid');
    expect(h.actions.providerLocked()).toBe(false);
  });
}

// Keep: one provider lock serializes search with Check key, Remove key and recording.
test('Find saved key holds the provider lock and is unavailable while busy, recording or configured', async () => {
  const h = await providerHandlers(false);
  h.actions.setRemovalDialog({ showModal() { h.calls.push('confirm'); }, close() {} });
  const gate = h.hold('find');
  const finding = h.actions.findSavedKey(h.id);
  await gate.started;
  expect(h.actions.finding()).toBe(h.id);
  expect(h.actions.providerLocked()).toBe(true);
  expect(h.actions.message(h.id)).toBe('Looking in Keychain');
  await h.actions.findSavedKey(h.id);
  await h.actions.checkAndSave(h.id);
  h.actions.requestRemoveKey(h.id);
  expect(h.calls).toEqual([`find:${h.id}`]);
  gate.release(); await finding;
  expect(h.actions.finding()).toBeNull();
  for (const state of ['recording', 'transcribing'] as const) {
    h.recording(state); h.calls.length = 0;
    await h.actions.findSavedKey(h.id);
    expect(h.calls).toEqual([]);
  }
  h.recording('idle');
  h.ctx.setApiKeyDraft(h.id, 'synthetic-input');
  await h.actions.verifyProvider(h.id);
  await h.actions.findSavedKey(h.id);
  expect(h.calls).toEqual([]);
  const configured = await providerHandlers(true);
  await configured.actions.findSavedKey(configured.id);
  expect(configured.calls).toEqual([]);
});

test.each(['provider', 'unmount'])('a %s change during Find saved key discards its message and focus', async (change) => {
  const h = await providerHandlers(false);
  await h.ctx.updateSettings({ providerKeyStatuses: { [h.id]: 'invalid' } });
  h.calls.length = 0;
  h.search('found');
  const gate = h.hold('find');
  const finding = h.actions.findSavedKey(h.id);
  await gate.started;
  if (change === 'provider') await h.ctx.updateSettings({ providerId: providerOptions[1].id });
  let next: ReturnType<typeof h.remount> | undefined;
  if (change === 'unmount') { h.destroy(); next = h.remount(); expect(next.actions.finding()).toBe(h.id); }
  gate.release(); await finding;
  expect(h.actions.message(h.id)).toBe('');
  expect(h.actions.tone(h.id)).toBeUndefined();
  expect(h.calls.some((call) => call.startsWith('focus:'))).toBe(false);
  // The native marker is committed either way; only the still-selected provider's warning resets.
  expect(h.ctx.settings().providerKeyStatuses?.[h.id]).toBe(change === 'unmount' ? 'unchecked' : 'invalid');
  expect(h.calls).toContain('refresh');
  expect(next?.actions.message(h.id)).toBeUndefined();
  expect((next ?? h).actions.providerLocked()).toBe(false);
});

// Keep: only a committed-marker failure may say a key was found; every rejection uses neutral copy.
test.each(['Saved key search failed. Try again.', 'window_not_authorized', 'runtime unavailable'])('a search rejection (%s) reports neutral failure copy', async (cause) => {
  const h = await providerHandlers(false);
  await h.ctx.updateSettings({ providerKeyStatuses: { [h.id]: 'invalid' } });
  h.calls.length = 0;
  if (cause === 'runtime unavailable') h.offline(true);
  else h.reject(cause);
  await h.actions.findSavedKey(h.id);
  expect(h.calls).toEqual([...(cause === 'runtime unavailable' ? [] : [`find:${h.id}`]), 'refresh', 'focus:find-key']);
  expect(h.actions.message(h.id)).toBe('Saved key search failed. Try again.');
  expect(h.actions.message(h.id)).not.toContain('Key found');
  expect(h.actions.tone(h.id)).toBe('error');
  expect(h.ctx.settings().providerKeyStatuses?.[h.id]).toBe('invalid');
  expect(h.actions.providerLocked()).toBe(false);
});

// Keep: the marker is committed once the search reports found, so follow-up failures keep the found result.
test.each(['reset', 'refresh'])('a failed %s after found keeps the found copy', async (failure) => {
  const h = await providerHandlers(false);
  await h.ctx.updateSettings({ providerKeyStatuses: { [h.id]: 'invalid' } });
  h.calls.length = 0;
  h.search('found');
  h.fail(failure);
  await h.actions.findSavedKey(h.id);
  expect(h.calls).toEqual([`find:${h.id}`, 'reset', 'refresh', failure === 'refresh' ? 'focus:find-key' : 'focus:check-key']);
  expect(h.actions.message(h.id)).toBe('Found your saved Groq key. Select Check key to test it.');
  expect(h.actions.tone(h.id)).toBe('success');
  expect(h.actions.configured(h.id)).toBe(failure !== 'refresh');
  expect(h.ctx.settings().providerKeyStatuses?.[h.id]).toBe(failure === 'reset' ? 'invalid' : 'unchecked');
  expect(h.actions.providerLocked()).toBe(false);
});
