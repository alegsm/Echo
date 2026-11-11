<script module lang="ts">
  import type { FnKeyPreference } from '#lib/runtime/desktop/fn-key-preference-client.ts';
  export type FnKeyInitialState = { preference?: FnKeyPreference; message?: string };
</script>

<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import { invoke, isTauri } from '@tauri-apps/api/core';
  import { createFnKeyPreferenceClient } from '#lib/runtime/desktop/fn-key-preference-client.ts';
  import { createFnKeyPreferenceController, type FnKeyState } from './fn-key-preference-controller';

  let { initialState = {} }: { initialState?: FnKeyInitialState } = $props();
  const initial = untrack(() => initialState);
  const viewState = $state<FnKeyState>({ preference: initial.preference, pending: false, message: initial.message ?? '' });
  let recheckButton = $state<HTMLButtonElement>();
  const controller = createFnKeyPreferenceController(createFnKeyPreferenceClient({ invoke }), viewState, {
    tick, activeElement: () => document.activeElement, focusRecheck: () => recheckButton?.focus(),
    openSettings: () => invoke('open_desktop_permission_settings', { kind: 'globe' }), isDesktop: isTauri
  });
  const { recognized, settingName, changePreference, openSettings } = controller;
  export const refresh = controller.refresh;
  onMount(controller.mount);
</script>

{#if viewState.preference?.supported}
  <div class="fn-control" role="group" aria-label="Fn key setting" aria-busy={viewState.pending}>
    <div class="purpose">
      <span class="name">Globe tap action</span>
      <p aria-live="polite">Current setting: {settingName(viewState.preference.current)}.</p>
      <p>Use Fn for Echo sets Do Nothing and saves your previous setting. Log out and back in if macOS still uses the previous action.</p>
      {#if !recognized(viewState.preference.current)}<p>Echo will not change an unrecognized value. Use Settings to review it.</p>{/if}
      {#if viewState.message}{#key viewState.alertRevision}<p role="alert">{viewState.message}</p>{/key}{/if}
    </div>
    <div class="actions">
      <button type="button" disabled={viewState.preference.current === 0 || !recognized(viewState.preference.current)} aria-disabled={viewState.pending} onclick={(event) => changePreference('use_fn', event.currentTarget)}>Use Fn for Echo</button>
      {#if viewState.preference.canRestore}
        <button type="button" aria-disabled={viewState.pending} onclick={(event) => changePreference('restore', event.currentTarget)}>Restore previous setting</button>
      {:else if viewState.preference.current !== null && recognized(viewState.preference.current)}
        <button type="button" aria-disabled={viewState.pending} onclick={(event) => changePreference('system_default', event.currentTarget)}>Use system default</button>
      {/if}
      <button type="button" class="recheck" bind:this={recheckButton} aria-disabled={viewState.pending} onclick={refresh}>Re-check Fn setting</button>
      <button type="button" onclick={openSettings}>Settings</button>
    </div>
  </div>
{:else if viewState.message}
  <div class="fn-control" role="group" aria-label="Fn key setting" aria-busy={viewState.pending}>
    <div class="purpose"><span class="name">Globe tap action</span>{#key viewState.alertRevision}<p role="alert">{viewState.message}</p>{/key}</div>
    <div class="actions"><button type="button" class="recheck" bind:this={recheckButton} aria-disabled={viewState.pending} onclick={refresh}>Re-check Fn setting</button></div>
  </div>
{/if}

<style>
  .fn-control { display: flex; align-items: center; gap: 14px; border-top: 1px solid var(--echo-divider); padding: 14px 0; }
  .purpose { flex: 1; min-width: 0; }
  .name { font: var(--t-label); color: var(--echo-fg); }
  p { margin: 3px 0 0; font: var(--t-meta); color: var(--echo-fg-3); }
  .actions { flex: 0 1 320px; display: flex; flex-wrap: wrap; justify-content: flex-end; gap: 10px; }
  button { flex: none; min-height: 30px; padding: 6px 12px; border: 0; border-radius: var(--r-bar); background: var(--echo-fill-1); color: var(--echo-fg); font: 500 12.5px/1 var(--font); cursor: pointer; }
  button:not(.recheck) { box-shadow: inset 0 0 0 1px var(--echo-hairline); }
  button.recheck { background: transparent; color: var(--echo-fg-3); }
  button:hover { background: var(--echo-fill-2); }
  button:disabled, button[aria-disabled="true"] { opacity: 0.45; cursor: default; }
  @media (max-width: 520px) { .fn-control { flex-wrap: wrap; gap: 10px; } .purpose { flex-basis: 100%; } .actions { flex-basis: 100%; justify-content: flex-start; } }
</style>
