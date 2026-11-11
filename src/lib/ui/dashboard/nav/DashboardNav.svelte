<script lang="ts">
  import { dashboardGlow, observeDashboardFrame, observeDashboardIntersection } from '../dashboard-motion';
  import EchoMark, { MARK_SHINE_PERIOD_MS } from '#lib/ui/brand/EchoMark.svelte';
  import { providerOptions } from '#lib/core/settings.ts';
  import { getDashboardContext, type SettingsTab } from '../context.js';
  import { applicablePermissionKinds } from '../panels/permissions-controller';

  let { activeTab, onselect, motionAllowed = true }: { motionAllowed?: boolean; activeTab: SettingsTab; onselect(tab: SettingsTab): void } = $props();
  let markVisible = $state(false);
  function observeMark(node: Element) {
    return observeDashboardIntersection(node, (visible) => { markVisible = visible; });
  }
  function observeMarkFrame(listener: FrameRequestCallback) {
    return observeDashboardFrame(document, listener);
  }
  const ctx = getDashboardContext();
  const pages = ['history', 'phrasebook', 'settings'] as const;
  const labels = ['History', 'Phrasebook', 'Settings'];
  let page = $derived(activeTab === 'setup' ? undefined : activeTab === 'history' || activeTab === 'phrasebook' ? activeTab : 'settings');
  let provider = $derived(providerOptions.find((item) => item.id === ctx.settings().providerId));
  let missing = $derived(ctx.permissions() ? applicablePermissionKinds(ctx.permissions()).some((kind) => !ctx.permissions()![kind]?.granted) : undefined);
  let permissionStatus = $derived(ctx.permissionCheckState() === 'unknown' ? 'Permissions unknown'
    : missing === undefined ? 'Checking permissions'
    : missing ? 'Permissions need attention' : 'Permissions granted');
</script>

<header class="dashboard-nav">
  <span {@attach observeMark} class="dashboard-brand" aria-hidden="true"><EchoMark motionAllowed={motionAllowed && markVisible} shinePeriodMs={MARK_SHINE_PERIOD_MS} observeFrame={observeMarkFrame} /><span class="brand-wordmark">Echo</span></span>
  <nav class="dashboard-destinations" aria-label="Echo">
    {#each pages as tab, i}
      <button {@attach dashboardGlow} type="button" class:echo-lit={page === tab} aria-label={labels[i]} title={labels[i]} aria-current={page === tab ? 'page' : undefined} onclick={() => onselect(tab)}>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          {#if tab === 'history'}<circle cx="12" cy="12" r="8.5" /><path d="M12 7.5V12l3 2" />
          {:else if tab === 'phrasebook'}<path d="M5 4.5h10.5a3 3 0 0 1 3 3v12H8a3 3 0 0 1-3-3zM5 16.5a3 3 0 0 1 3-3h10.5M9 8h6" />
          {:else}<path d="M4 7h9M17 7h3M4 17h3M11 17h9" /><circle cx="15" cy="7" r="2" /><circle cx="9" cy="17" r="2" />{/if}
        </svg><span class="nav-label">{labels[i]}</span>
      </button>
    {/each}
  </nav>
  <button class="dashboard-status" type="button" onclick={() => onselect('permissions')} title={`${ctx.settings().pushToTalkShortcut || 'None'} · ${provider?.label} · ${permissionStatus}`} aria-label={`Shortcut ${ctx.settings().pushToTalkShortcut || 'None'}, ${provider?.label}. ${permissionStatus}`}>
    <span class="status-copy"><kbd>{ctx.settings().pushToTalkShortcut || 'None'}</kbd></span>
    <span aria-hidden="true" class="status-dot" class:needs-attention={ctx.permissionCheckState() === 'unknown' || missing !== false}></span><span class="status-copy provider-label">{provider?.label}</span>
  </button>
</header>

<style>
  .dashboard-nav {
    grid-column: 1;
    grid-row: 1;
    z-index: 20;
    min-height: 0;
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 24px;
    margin: 12px 0 12px 12px;
    padding: 20px 12px;
    border-radius: var(--r-card);
    background: var(--echo-bar);
    box-shadow: inset 0 0 0 1px var(--echo-hairline);
    -webkit-backdrop-filter: var(--echo-frost);
    backdrop-filter: var(--echo-frost);
    overflow-y: auto;
    scrollbar-width: none;
  }
  .dashboard-brand { display: flex; align-items: center; justify-content: center; gap: 10px; flex: none; }
  .brand-wordmark { font: 500 13px/1.3 var(--font); color: var(--echo-fg); }
  .dashboard-destinations { display: flex; flex-direction: column; gap: 10px; padding: 4px; }
  button { border: 0; cursor: pointer; color: var(--echo-fg-3); font: 500 13px/1.3 var(--font); }
  .dashboard-destinations button {
    min-height: var(--bar-h);
    border-radius: var(--r-bar);
    padding: 10px 12px;
    background: transparent;
    display: flex;
    align-items: center;
    gap: 10px;
    text-align: left;
  }
  .dashboard-destinations svg { flex: none; }
  .dashboard-destinations button:hover, .dashboard-destinations button[aria-current] { color: var(--echo-fg); }
  .dashboard-destinations button[aria-current] { background: var(--echo-fill-3); }
  .dashboard-status {
    margin-top: auto;
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    align-items: center;
    gap: 8px;
    padding: 12px 8px;
    border-radius: var(--r-bar);
    background: transparent;
    overflow-wrap: anywhere;
  }
  .status-copy:first-child { width: 100%; }
  .provider-label { min-width: 0; }
  kbd { display: inline-block; max-width: 100%; font: 400 11px/1.3 var(--mono); padding: 4px 7px; border-radius: var(--r-bar); background: var(--echo-fill-2); overflow-wrap: anywhere; }
  .status-dot { width: 7px; height: 7px; flex: none; border-radius: 50%; background: var(--tone-success); }
  .needs-attention { background: var(--tone-handsfree); }
  @media (max-width: 719px) {
    .dashboard-nav { margin-left: 8px; padding: 16px 4px; gap: 20px; }
    .dashboard-destinations { padding: 4px; }
    .dashboard-destinations button { justify-content: center; padding: 10px 0; }
    .nav-label, .brand-wordmark { display: none; }
    .status-copy { display: none; }
    .dashboard-status { min-height: 44px; padding: 12px 0; }
  }
</style>
