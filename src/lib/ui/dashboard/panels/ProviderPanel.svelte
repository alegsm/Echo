<script lang="ts">
  import { dashboardGlow } from '../dashboard-motion';
  import { onDestroy, tick } from 'svelte';
  import { providerOptions } from '#lib/core/settings.ts';
  import { getDashboardContext } from '../context.js';
  import { createProviderController, type ProviderState } from './provider-controller';
  import { desktopPlatform } from './permissions-controller';
  import DashboardCard from '../layout/DashboardCard.svelte';
  import ProviderCheckLight from './ProviderCheckLight.svelte';

  const ctx = getDashboardContext();
  let platform = $derived(desktopPlatform(ctx.permissions()));
  let credentialStore = $derived(platform === 'windows' ? 'Credential Manager' : 'Keychain');
  let provider = $derived(providerOptions.find((p) => p.id === ctx.settings().providerId)!);
  const viewState = $state<ProviderState>({
    operation: { busy: false, checking: null, finding: null }, messages: {}, resultTones: {}, removalProvider: null
  });
  let removalDialog = $state<HTMLDialogElement | null>(null);
  let saveDialog = $state<HTMLDialogElement | null>(null);
  let checkButton = $state<HTMLButtonElement | null>(null);
  let findButton = $state<HTMLButtonElement | null>(null);
  // Search results move focus only when it was lost with the disabled or unmounted button.
  const focusUnclaimed = () => [null, document.body, findButton].includes(document.activeElement as HTMLButtonElement | null);
  const { providerLocked, configured, removable, warning, selectProvider,
    trapDialogFocus, requestRemoveKey, removeProviderKey, closeRemovalDialog, verifyProvider, confirmSave, cancelSave, pendingCurrent, invalidatePendingSave, findSavedKey, dispose } = createProviderController(ctx, viewState, {
    tick, dialog: () => removalDialog, saveDialog: () => ({
      showModal() { saveDialog?.showModal(); saveDialog?.querySelector<HTMLButtonElement>('button')?.focus(); },
      close() { saveDialog?.close(); }
    }), activeElement: () => document.activeElement,
    focusKey: () => document.getElementById('provider-key')?.focus(),
    focusCheck: () => { if (focusUnclaimed()) checkButton?.focus(); },
    focusFind: () => { if (focusUnclaimed()) findButton?.focus(); }
  });
  let checking = $derived(viewState.operation.checking === provider.id);
  let finding = $derived(viewState.operation.finding === provider.id);
  let pendingMessage = $derived(checking ? `Checking key with ${provider.label}` : finding ? `Looking in ${credentialStore}` : '');
  $effect(() => {
    if (viewState.pendingSave && !pendingCurrent()) invalidatePendingSave();
  });
  onDestroy(dispose);
</script>

<section aria-labelledby="settings-provider">
  <DashboardCard padding="var(--card-pad)">
    <div class="card-head"><h2 id="settings-provider" tabindex="-1">Provider</h2><span>One model each</span></div>
    <div class="providers" role="group" aria-label="Provider">
      {#each providerOptions as option}
        <button {@attach dashboardGlow} id="provider-{option.id}" type="button" class="provider" class:echo-lit={provider.id === option.id} aria-pressed={provider.id === option.id} disabled={providerLocked() && provider.id !== option.id} onclick={() => void selectProvider(option.id)}>
          <span><b>{option.label}</b><small>{option.model}</small>{#if configured(option.id)}<small>Configured</small>{/if}</span>
          {#if warning(option.id)}<span class="warning" role="img" aria-label="Credential needs attention">⚠</span>
          {:else if provider.id === option.id}<svg class="selected-mark" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="m5 12 4 4L19 6" /></svg>{/if}
        </button>
      {/each}
    </div>
    <div class="credential-row">
      <div class="field-wrap">
        <label class="sr-only" for="provider-key">{provider.label} API key</label>
        <input id="provider-key" type="password" autocomplete="off" spellcheck="false" disabled={providerLocked()} placeholder="Paste your {provider.label} key" value={ctx.apiKeyDrafts()[provider.id]} oninput={(event) => ctx.setApiKeyDraft(provider.id, event.currentTarget.value)} />
      </div>
      {#if warning(provider.id)}<span class="warning" role="img" aria-label="Credential needs attention">⚠</span>{/if}
      <button bind:this={checkButton} type="button" class="key-action check-key" class:running={checking} aria-busy={checking} disabled={providerLocked()} onclick={() => void verifyProvider(provider.id)}>{#if checking}<ProviderCheckLight />{/if}<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true"><circle cx="8" cy="16" r="4" /><path d="m11 13 9-9m-3 3 3 3" /></svg>Check key</button>
    </div>
    {#if removable(provider.id)}
      <button type="button" class="key-action remove-key" disabled={providerLocked()} onclick={() => requestRemoveKey(provider.id)}><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M4 7h16M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2M6.5 7l.8 12.1a2 2 0 0 0 2 1.9h5.4a2 2 0 0 0 2-1.9L17.5 7M10 11v6M14 11v6" /></svg>Remove key</button>
    {:else}
      <div class="find-saved">
        <button bind:this={findButton} type="button" class="key-action find-key" class:running={finding} aria-busy={finding} disabled={providerLocked()} onclick={() => void findSavedKey(provider.id)}>{#if finding}<ProviderCheckLight />{/if}<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true" focusable="false"><circle cx="10.5" cy="10.5" r="6" /><path d="m15 15 5 5" /></svg>Find saved key</button>
        {#if platform === 'macos'}<span>macOS may ask you to allow Echo access.</span>{/if}
      </div>
    {/if}
    <p class="check-result" role="status">{#if pendingMessage || viewState.messages[provider.id]}{#if pendingMessage || viewState.resultTones[provider.id]}<span class="result-dot" class:loading={!!pendingMessage} class:success={viewState.resultTones[provider.id] === 'success'} class:error={viewState.resultTones[provider.id] === 'error'} aria-hidden="true"></span>{/if}{pendingMessage || viewState.messages[provider.id]}{/if}</p>
  </DashboardCard>
</section>

<dialog bind:this={saveDialog} aria-labelledby="save-key-heading" onkeydown={trapDialogFocus} oncancel={(event) => { event.preventDefault(); void cancelSave(); }} onclose={() => void cancelSave()}>
  <h3 id="save-key-heading">Save {provider.label} key</h3>
  <p>Echo will save your key in {credentialStore === 'Keychain' ? 'macOS Keychain' : 'Windows Credential Manager'} on this device. Echo reads it only to call {provider.label}.</p>
  {#if credentialStore === 'Keychain'}<p>macOS may ask you to allow Echo access.</p>{/if}
  <div class="dialog-actions">
    <button type="button" onclick={() => void cancelSave()}>Cancel</button>
    <button type="button" onclick={() => void confirmSave()}>Save to {credentialStore}</button>
  </div>
</dialog>

<dialog bind:this={removalDialog} aria-label="Remove saved key" onkeydown={trapDialogFocus} oncancel={(event) => { if (viewState.operation.busy) event.preventDefault(); }} onclose={() => void closeRemovalDialog()}>
  <p>Remove the saved {providerOptions.find((p) => p.id === viewState.removalProvider)?.label ?? ''} key from {credentialStore}?</p>
  <div class="dialog-actions">
    <button type="button" disabled={viewState.operation.busy} onclick={() => removalDialog?.close()}>Cancel</button>
    <button type="button" disabled={viewState.operation.busy} onclick={() => void removeProviderKey()}>Remove saved key</button>
  </div>
</dialog>

<style>
  section { grid-column: 1 / -1; min-width: 0; }
  .card-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 12px; }
  h2 { margin: 0; font: var(--t-caps); letter-spacing: 0.08em; text-transform: uppercase; color: var(--echo-fg-3); }
  .card-head > span { font: var(--t-meta); color: var(--echo-fg-3); }
  button { border: 0; color: var(--echo-fg); cursor: pointer; font: 500 13px/1 var(--font); }
  button:disabled, input:disabled { opacity: 0.45; cursor: default; }
  .providers { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; padding: 12px 0; }
  .provider { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 11px 14px; border-radius: 14px; background: var(--echo-well); box-shadow: inset 0 0 0 1px var(--echo-hairline); text-align: left; min-width: 0; }
  .provider[aria-pressed='true'] { background: var(--echo-fill-2); box-shadow: none; }
  .provider b { display: block; font: 500 13.5px/1.2 var(--font); }
  .provider small { display: block; margin-top: 3px; font: 400 10.5px/1.2 var(--mono); color: var(--echo-fg-2); overflow-wrap: anywhere; }
  .selected-mark { color: var(--tone-record); flex: none; }
  .credential-row { display: flex; align-items: center; gap: 8px; }
  .field-wrap { min-width: 0; flex: 1; }
  input { box-sizing: border-box; width: 100%; min-width: 0; height: 38px; padding: 0 14px; border: 0; border-radius: var(--r-control); background: var(--echo-well); box-shadow: inset 0 0 0 1px var(--echo-hairline); color: var(--echo-fg); font: var(--t-body); }
  input::placeholder { color: var(--echo-fg-3); }
  .warning { color: var(--tone-handsfree); flex: none; }
  .key-action, dialog button { min-height: 34px; padding: 0 16px; border-radius: var(--r-bar); background: var(--echo-fill-1); box-shadow: inset 0 0 0 1px var(--echo-hairline); }
  .key-action { position: relative; display: inline-flex; align-items: center; justify-content: center; gap: 7px; white-space: nowrap; }
  .key-action.running { opacity: 1; color: var(--echo-fg-2); box-shadow: none; cursor: progress; }
  button:hover { background: var(--echo-fill-2); }
  /* Same slot as .find-saved: its top spacing, start-aligned in the card's column. */
  .remove-key { align-self: flex-start; margin-top: 8px; }
  /* A quiet destructive cue: the icon carries the error tint, and the label joins it on hover. */
  .remove-key svg { color: var(--tone-error-text); }
  .remove-key:hover:not(:disabled) { color: var(--tone-error-text); }
  .find-saved { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 12px; margin-top: 8px; }
  .find-saved span { flex: 1 1 12em; min-width: 0; font: var(--t-meta); color: var(--echo-fg-3); }
  .dialog-actions { display: flex; flex-wrap: wrap; gap: 8px; }
  .result-dot { width: 6px; height: 6px; flex: none; border-radius: 50%; }
  .result-dot.loading { background: var(--tone-loading); box-shadow: 0 0 6px color-mix(in srgb, var(--tone-loading) 60%, transparent); }
  .result-dot.success { background: var(--tone-success); }
  .result-dot.error { background: var(--tone-error); }
  .check-result { display: flex; align-items: center; gap: 7px; margin: 10px 0 0; font: var(--t-meta); color: var(--echo-fg-2); }
  .check-result:empty { display: none; }
  .sr-only { position: absolute; width: 1px; height: 1px; padding: 0; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  dialog { box-sizing: border-box; max-width: min(380px, calc(100vw - 32px)); padding: var(--card-pad); border: 1px solid var(--echo-hairline); border-radius: var(--r-card); background: var(--echo-canvas); color: var(--echo-fg); }
  dialog::backdrop { background: rgba(0, 0, 0, 0.6); }
  dialog h3 { margin: 0 0 12px; font: var(--t-body); font-weight: 600; }
  dialog p { margin: 0 0 16px; font: var(--t-body); }
  @media (max-width: 479px) { .providers { grid-template-columns: minmax(0, 1fr); } .credential-row { flex-wrap: wrap; } .field-wrap { flex-basis: calc(100% - 32px); } }
</style>
