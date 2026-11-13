<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke, isTauri } from '@tauri-apps/api/core';
  import { createLoginItemClient } from '#lib/runtime/desktop/login-item-client.ts';
  import { getDashboardContext } from '../context.js';
  import { desktopPlatform } from './permissions-controller';
  import { createBackgroundController, type BackgroundState } from './background-controller';
  import DashboardCard from '../layout/DashboardCard.svelte';

  const ctx = getDashboardContext();
  const initialState = $state<BackgroundState>({ saving: false, loginError: null, registered: null });
  const keepRunningHelp = $derived(desktopPlatform(ctx.permissions()) === 'windows'
    ? 'Echo stays in the notification area. Quit from its icon.'
    : 'Echo stays in the menu bar. Quit from there or with Cmd+Q.');
  const { state: viewState, toggle, mismatch, mount } = createBackgroundController(ctx, initialState,
    isTauri() ? createLoginItemClient({ invoke }) : undefined);
  const mismatchLine = $derived(mismatch());
  onMount(mount);
</script>

<section aria-labelledby="settings-background">
  <DashboardCard padding="var(--card-pad)">
    <h2 id="settings-background">Background</h2>
    <div class="background-row">
      <div><span>Keep Echo running when the dashboard closes</span><p>{keepRunningHelp}</p></div>
      <button type="button" class="echo-switch" role="switch" aria-checked={ctx.settings().runInBackground} aria-label="Keep Echo running when the dashboard closes" aria-disabled={viewState.saving} onclick={() => void toggle('runInBackground')}></button>
    </div>
    <div class="background-row">
      <div>
        <span>Start Echo at login</span><p>Opens Echo when you sign in.</p>
        {#if mismatchLine}<p>{mismatchLine}</p>{/if}
        {#if viewState.loginError}<p class="error" role="alert">{viewState.loginError}</p>{/if}
      </div>
      <button type="button" class="echo-switch" role="switch" aria-checked={ctx.settings().launchAtLogin} aria-label="Start Echo at login" aria-disabled={viewState.saving} onclick={() => void toggle('launchAtLogin')}></button>
    </div>
  </DashboardCard>
</section>

<style>
  section { grid-column: 1 / -1; min-width: 0; }
  h2 { margin: 0 0 12px; font: var(--t-caps); letter-spacing: 0.08em; text-transform: uppercase; color: var(--echo-fg-3); }
  .background-row { display: flex; align-items: center; justify-content: space-between; gap: 16px; padding: 13px 0; }
  .background-row + .background-row { border-top: 1px solid var(--echo-divider); }
  .background-row > div { min-width: 0; }
  .background-row span { font: var(--t-label); color: var(--echo-fg); }
  .background-row p { margin: 3px 0 0; font: var(--t-meta); color: var(--echo-fg-3); }
  /* TranscriptionPanel's error rule, scoped to the row so it beats the help-text colour above. */
  .background-row .error { color: var(--tone-error-text); overflow-wrap: anywhere; }
  /* Pending saves use aria-disabled so the switch keeps keyboard focus; match the shared :disabled look. */
  .echo-switch[aria-disabled="true"] { opacity: 0.45; cursor: default; }
</style>
