import { providerOptions, type ProviderId } from '#lib/core/settings.ts';
import type { EchoRuntime, EchoSnapshot, ProviderKeySearchOutcome } from '#lib/core/types.ts';
import type { DashboardContext } from '../context';
import { desktopPlatform } from './permissions-controller';

export type ProviderOperation = { busy: boolean; checking: ProviderId | null; finding?: ProviderId | null };
export type ProviderState = {
  operation: ProviderOperation;
  messages: Partial<Record<ProviderId, string>>;
  resultTones: Partial<Record<ProviderId, 'success' | 'error'>>;
  removalProvider: ProviderId | null;
  pendingSave?: { id: ProviderId; draft: string } | null;
};
type ProviderContext = Pick<DashboardContext, 'settings' | 'apiKeyDrafts'
  | 'providerKeyPresence' | 'setActionError' | 'setApiKeyDraft' | 'updateSettings' | 'refreshProviderKeyPresence'> & {
  runtime: () => Pick<EchoRuntime, 'setProviderApiKey' | 'deleteProviderApiKey' | 'findProviderApiKey' | 'probeProviderKey'> | undefined;
  snapshot: () => Pick<EchoSnapshot, 'state'>;
} & Partial<Pick<DashboardContext, 'permissions'>>;
type ProviderEffects = {
  tick: () => Promise<void>;
  dialog: () => Pick<HTMLDialogElement, 'showModal' | 'close'> | null | undefined;
  saveDialog?: () => Pick<HTMLDialogElement, 'showModal' | 'close'> | null | undefined;
  activeElement: () => Element | null;
  focusKey: () => void;
  focusCheck?: () => void;
  focusFind?: () => void;
};
type ResultTone = ProviderState['resultTones'][ProviderId];

// Context identity keeps the reactive operation lock alive across panel remounts.
const operations = new WeakMap<ProviderContext, ProviderOperation>();

export function createProviderController(ctx: ProviderContext, state: ProviderState, effects: ProviderEffects) {
  state.operation = operations.get(ctx) ?? state.operation;
  operations.set(ctx, state.operation);
  let active = true;

  function providerLocked() {
    return state.operation.busy || ['requesting_microphone', 'recording', 'stopping', 'transcribing', 'inserting'].includes(ctx.snapshot().state);
  }

  function configured(id: ProviderId) {
    // Native presence reports metadata without opening the credential store.
    return ctx.providerKeyPresence()[id];
  }

  function removable(id: ProviderId) {
    return configured(id);
  }

  function warning(id: ProviderId) {
    return ctx.settings().providerKeyStatuses?.[id] === 'invalid';
  }

  async function selectProvider(id: ProviderId) {
    if (providerLocked()) return;
    try {
      if (ctx.settings().providerId !== id) {
        await ctx.updateSettings({ providerId: id });
      }
    } catch {
      ctx.setActionError('Provider could not be saved. Try again.');
    }
  }

  function trapDialogFocus(event: KeyboardEvent) {
    if (event.key !== 'Tab') return;
    const controls = [...(event.currentTarget as HTMLDialogElement).querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
    const index = controls.indexOf(effects.activeElement() as HTMLButtonElement);
    const next = (index + (event.shiftKey ? -1 : 1) + controls.length) % controls.length;
    event.preventDefault();
    controls[next]?.focus();
  }

  function requestRemoveKey(id: ProviderId) {
    if (providerLocked() || !removable(id)) return;
    state.removalProvider = id;
    effects.dialog()?.showModal();
  }

  async function removeProviderKey() {
    const id = state.removalProvider;
    if (!id || providerLocked() || !removable(id)) return;
    state.operation.busy = true;
    const draft = ctx.apiKeyDrafts()[id];
    try {
      const runtime = ctx.runtime();
      if (!runtime) throw new Error('Runtime unavailable');
      await runtime.deleteProviderApiKey(id);
      if (ctx.apiKeyDrafts()[id] === draft) ctx.setApiKeyDraft(id, '');
      await ctx.updateSettings({ providerKeyStatuses: { [id]: 'unchecked' } });
      await ctx.refreshProviderKeyPresence();
      state.messages[id] = '';
      delete state.resultTones[id];
    } catch {
      state.messages[id] = 'Key could not be removed. Try again.';
      state.resultTones[id] = 'error';
    } finally {
      state.operation.busy = false;
      state.removalProvider = null;
      effects.dialog()?.close();
    }
  }

  function credentialStore() {
    return desktopPlatform(ctx.permissions?.()) === 'windows' ? 'Credential Manager' : 'Keychain';
  }

  function searchResult(outcome: ProviderKeySearchOutcome | 'failed', label: string): [string, ResultTone?] {
    const store = credentialStore();
    switch (outcome) {
      case 'found': return [`Found your saved ${label} key. Select Check key to test it.`, 'success'];
      case 'not_found': return [`No saved ${label} key in ${store}. Paste your key and select Check key.`];
      case 'invalid': return [`The saved ${label} key in ${store} can't be used. Paste your key and select Check key.`, 'error'];
      case 'unavailable': return [store === 'Credential Manager'
        ? 'Credential Manager could not be read. Try again.'
        : 'Keychain access was denied or unavailable. Select Find saved key again and choose Allow.', 'error'];
      case 'already_saved': return [''];
      case 'save_failed': return ['Key found, but Echo could not save that. Try again.', 'error'];
      default: return ['Saved key search failed. Try again.', 'error'];
    }
  }

  // Reads the OS store only on this explicit request; the key itself never reaches the renderer.
  async function findSavedKey(id: ProviderId) {
    if (providerLocked() || !active || configured(id)) return;
    state.operation.busy = true;
    state.operation.finding = id;
    const label = providerOptions.find((p) => p.id === id)!.label;
    const selected = () => ctx.settings().providerId === id;
    const current = () => active && selected();
    state.messages[id] = `Looking in ${credentialStore()}`;
    delete state.resultTones[id];
    let outcome: ProviderKeySearchOutcome | 'failed' = 'failed';
    try {
      try {
        const runtime = ctx.runtime();
        if (runtime) outcome = await runtime.findProviderApiKey(id);
      } catch { /* Any rejection means no result exists; it never claims a key was found. */ }
      // The marker is already committed after `found`; follow-up failures cannot change that result.
      if (outcome === 'found' && selected()) {
        await ctx.updateSettings({ providerKeyStatuses: { [id]: 'unchecked' } }).catch(() => undefined);
      }
      if (outcome === 'found' || outcome === 'already_saved' || outcome === 'failed') {
        await ctx.refreshProviderKeyPresence().catch(() => undefined);
      }
    } finally {
      state.operation.finding = null;
      state.operation.busy = false;
    }
    const [message, tone] = current() ? searchResult(outcome, label) : [''];
    state.messages[id] = message;
    if (tone) state.resultTones[id] = tone;
    else delete state.resultTones[id];
    if (!current()) return;
    await effects.tick();
    if (!current()) return;
    // Find saved key unmounts once presence reports the key, so focus moves to the next action.
    if (configured(id)) effects.focusCheck?.();
    else effects.focusFind?.();
  }

  async function closeRemovalDialog() {
    state.removalProvider = null;
    await effects.tick();
    effects.focusKey();
  }

  function pendingCurrent() {
    const pending = state.pendingSave;
    return active && !!pending && ctx.settings().providerId === pending.id
      && ctx.apiKeyDrafts()[pending.id] === pending.draft;
  }

  function invalidatePendingSave() {
    if (!state.pendingSave) return;
    state.pendingSave = null;
    state.operation.busy = false;
    effects.saveDialog?.()?.close();
  }

  async function cancelSave() {
    if (!state.pendingSave) return;
    invalidatePendingSave();
    await effects.tick();
    if (active) effects.focusKey();
  }

  async function confirmSave() {
    const pending = state.pendingSave;
    if (!pendingCurrent() || !pending) { invalidatePendingSave(); return; }
    state.pendingSave = null;
    effects.saveDialog?.()?.close();
    await runVerification(pending.id);
  }

  async function verifyProvider(id: ProviderId) {
    if (providerLocked() || !active) return;
    const draft = ctx.apiKeyDrafts()[id];
    if (draft.trim()) {
      state.pendingSave = { id, draft };
      state.operation.busy = true;
      effects.saveDialog?.()?.showModal();
      return;
    }
    await runVerification(id);
  }

  async function runVerification(id: ProviderId) {
    state.operation.busy = true;
    state.operation.checking = id;
    const selected = providerOptions.find((p) => p.id === id)!;
    let expectedDraft = ctx.apiKeyDrafts()[id];
    const current = () => ctx.settings().providerId === id
      && ctx.apiKeyDrafts()[id] === expectedDraft;
    state.messages[id] = `Checking key with ${selected.label}`;
    delete state.resultTones[id];
    try {
      const runtime = ctx.runtime();
      if (!runtime) throw new Error('Runtime unavailable');
      const draft = expectedDraft.trim();
      if (draft) {
        await runtime.setProviderApiKey(id, draft);
        if (!current()) return;
        ctx.setApiKeyDraft(id, '');
        expectedDraft = '';
      }
      if (draft) await ctx.updateSettings({ providerKeyStatuses: { [id]: 'unchecked' } });
      if (!current()) return;
      await runtime.probeProviderKey({ providerId: id });
      if (!current()) return;
      await ctx.updateSettings({ providerKeyStatuses: { [id]: 'valid' } });
      if (!current()) return;
      await ctx.refreshProviderKeyPresence();
      if (!current()) return;
      if (!active) return;
      state.messages[id] = `Key works, checked at ${new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`;
      state.resultTones[id] = 'success';
    } catch (error: unknown) {
      if (!current()) return;
      const code = error && typeof error === 'object' && 'code' in error ? error.code : undefined;
      const credentialError = ['missing_provider_key', 'credential_unavailable', 'provider_credential_rejected'].includes(String(code));
      if (credentialError) {
        try { await ctx.updateSettings({ providerKeyStatuses: { [id]: 'invalid' } }); }
        catch { /* Keep retry available when the warning cannot be persisted. */ }
      }
      if (!current()) return;
      await ctx.refreshProviderKeyPresence();
      if (!current()) return;
      if (!active) return;
      state.resultTones[id] = 'error';
      state.messages[id] = credentialError ? 'Key unavailable or rejected. Update it and try again.' : 'Key check failed. Try again.';
    } finally {
      if (!active || !current()) {
        state.messages[id] = '';
        delete state.resultTones[id];
      }
      state.operation.checking = null;
      state.operation.busy = false;
    }
  }
  return {
    providerLocked, configured, removable, warning, selectProvider,
    trapDialogFocus, requestRemoveKey, removeProviderKey, closeRemovalDialog, verifyProvider,
    confirmSave, cancelSave, pendingCurrent, invalidatePendingSave, findSavedKey,
    dispose() { active = false; invalidatePendingSave(); }
  };
}
