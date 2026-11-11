<script lang="ts">
  import { dashboardGlow } from '../dashboard-motion';
  import { tick } from 'svelte';
  import { getDashboardContext } from '../context.js';
  import { applicablePermissionKinds, desktopPlatform, createPermissionsController, type PermissionsState } from './permissions-controller';
  import DashboardCard from '../layout/DashboardCard.svelte';
  import FnKeyPreferenceControl from './FnKeyPreferenceControl.svelte';

  const ctx = getDashboardContext();
  const allRows = [
    { kind: 'microphone', label: 'Microphone', detail: 'Capture your voice for dictation.', icon: 'M12 15a3 3 0 0 0 3-3V5a3 3 0 0 0-6 0v7a3 3 0 0 0 3 3ZM5 10v2a7 7 0 0 0 14 0v-2M12 19v3m-4 0h8' },
    { kind: 'accessibility', label: 'Accessibility', detail: 'Paste transcripts into your active app.', icon: 'M12 8v6m-7-4 7 2 7-2m-7 4-4 7m4-7 4 7M14 4a2 2 0 1 1-4 0 2 2 0 0 1 4 0Z' },
    { kind: 'input_monitoring', label: 'Input Monitoring', detail: 'Listen for your global recording shortcut.', icon: 'M3 5h18v14H3ZM7 9h.01M11 9h.01M15 9h.01M7 13h.01M11 13h.01M15 13h.01M7 16h10' }
  ] as const;
  let platform = $derived(desktopPlatform(ctx.permissions()));
  let rows = $derived(allRows.filter((row) => applicablePermissionKinds(ctx.permissions()).includes(row.kind)));
  const viewState = $state<PermissionsState>({ permissionRefreshState: 'idle', permissionAction: undefined });
  let fnControl = $state<FnKeyPreferenceControl>();
  let firstPending = $derived(rows.find((row) => ctx.permissions()?.[row.kind]?.granted === false)?.kind);
  let resultsReady = $derived(ctx.permissionCheckState() === 'ready');
  let complete = $derived(resultsReady && rows.every((row) => ctx.permissions()?.[row.kind]));
  let granted = $derived(rows.filter((row) => ctx.permissions()?.[row.kind]?.granted).length);

  const { doRefresh, requestPermission, openPermissionSettings } = createPermissionsController(ctx, viewState, {
    tick, activeElement: () => document.activeElement,
    focusHeading: () => document.getElementById('settings-permissions')?.focus(),
    refreshFn: async () => { await fnControl?.refresh(); }
  });
</script>

<section aria-labelledby="settings-permissions">
  <DashboardCard padding="var(--card-pad)">
    <div class="card-head">
      <h2 id="settings-permissions" tabindex="-1">Permissions</h2>
      <span role="status">{#if complete}{granted}/{rows.length} granted{/if}</span>
      <button type="button" class="recheck" aria-disabled={viewState.permissionRefreshState === 'checking' || !!viewState.permissionAction} onclick={doRefresh}><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 7v5h-5M4 17v-5h5M6 7a7 7 0 0 1 12-1l2 6M4 12l2 6a7 7 0 0 0 12-1" /></svg>{viewState.permissionRefreshState === 'checking' ? 'Checking…' : 'Re-check'}</button>
    </div>
    <ul aria-label="Desktop permissions">
      {#each rows as row (row.kind)}
        {@const item = ctx.permissions()?.[row.kind]}
        {@const unknown = item?.status === 'unknown' || item?.status === 'unsupported'}
        {@const settingsAction = unknown || platform === 'windows'}
        <li>
          <span class="permission-icon"><svg width="20" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={row.icon} /></svg></span>
          <div class="purpose"><span class="name">{row.label}</span><p>{row.detail}</p></div>
          <span class="state" class:granted={resultsReady && item?.granted} class:denied={resultsReady && item?.granted === false}><span class="state-dot" aria-hidden="true"></span>{ctx.permissionCheckState() === 'checking' ? 'Checking…' : !resultsReady || !item || unknown ? 'Unknown' : item.granted ? 'Granted' : 'Not granted'}</span>
          {#if resultsReady && item?.granted === false}
            <button {@attach dashboardGlow} type="button" class:echo-lit={row.kind === firstPending} aria-label="{settingsAction ? 'Settings' : 'Grant'} {row.label}" aria-disabled={!!viewState.permissionAction || viewState.permissionRefreshState === 'checking'} onclick={(event) => settingsAction ? openPermissionSettings(row.kind) : requestPermission(row.kind, event.currentTarget)}>{viewState.permissionAction === row.kind ? 'Working…' : settingsAction ? 'Settings' : 'Grant'}</button>
          {/if}
        </li>
      {/each}
    </ul>
    {#if platform === 'macos'}<FnKeyPreferenceControl bind:this={fnControl} />{/if}
  </DashboardCard>
</section>

<style>
  section { grid-column: 1 / -1; min-width: 0; }
  .card-head { display: flex; align-items: center; flex-wrap: wrap; gap: 12px; margin-bottom: 8px; }
  h2 { margin: 0; font: var(--t-caps); letter-spacing: 0.08em; text-transform: uppercase; color: var(--echo-fg-3); }
  .card-head > span { margin-left: auto; font: var(--t-meta); color: var(--echo-fg-3); }
  ul { list-style: none; margin: 0; padding: 0; }
  li { display: flex; align-items: center; gap: 14px; padding: 14px 0; }
  li + li { border-top: 1px solid var(--echo-divider); }
  .permission-icon { display: grid; place-items: center; width: 34px; height: 34px; flex: none; border-radius: 50%; box-shadow: inset 0 0 0 1px var(--echo-hairline); color: var(--echo-fg-2); }
  .recheck { display: inline-flex; align-items: center; gap: 6px; }
  .purpose { flex: 1; min-width: 0; }
  .name { font: var(--t-label); color: var(--echo-fg); }
  p { margin: 3px 0 0; font: var(--t-meta); color: var(--echo-fg-3); }
  .state { display: inline-flex; align-items: center; gap: 6px; font: var(--t-meta); color: var(--echo-fg-3); white-space: nowrap; }
  .state-dot { width: 7px; height: 7px; border-radius: 50%; background: currentColor; }
  .state.denied { color: var(--tone-handsfree); }
  .state.granted { color: var(--tone-success); }
  button { flex: none; min-height: 30px; padding: 0 12px; border: 0; border-radius: var(--r-bar); background: var(--echo-fill-1); color: var(--echo-fg); font: 500 12.5px/1 var(--font); cursor: pointer; }
  button:not(.echo-lit, .recheck) { box-shadow: inset 0 0 0 1px var(--echo-hairline); }
  button.recheck { background: transparent; color: var(--echo-fg-3); }
  button:hover { background: var(--echo-fill-2); }
  button:disabled, button[aria-disabled="true"] { opacity: 0.45; cursor: default; }
  @media (max-width: 520px) { li { flex-wrap: wrap; gap: 10px; } .purpose { flex-basis: calc(100% - 44px); } .state { margin-left: 44px; } }
</style>
