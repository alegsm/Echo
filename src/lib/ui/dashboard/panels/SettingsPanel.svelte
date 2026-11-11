<script lang="ts">
  import PageHeader from '../layout/PageHeader.svelte';
  import KeybindsPanel from './KeybindsPanel.svelte';
  import ProviderPanel from './ProviderPanel.svelte';
  import TranscriptionPanel from './TranscriptionPanel.svelte';
  import PermissionsPanel from './PermissionsPanel.svelte';
  import DebugLogRow from './DebugLogRow.svelte';
  import { getDashboardContext } from '../context.js';
  const ctx = getDashboardContext();
</script>

<div class="settings-page">
  <PageHeader title="Settings" />
  <button type="button" class="setup-entry" onclick={() => ctx.setActiveTab('setup')}>{ctx.setupRequired() ? 'Finish setup to start transcribing' : 'Run setup again'}</button>
  <div class="settings-grid">
    <KeybindsPanel />
    <ProviderPanel />
    <TranscriptionPanel />
    <PermissionsPanel />
    <DebugLogRow />
  </div>
</div>

<style>
  .setup-entry { align-self: flex-start; padding: 6px 4px; border: 0; background: transparent; color: var(--echo-fg-2); font: var(--t-meta); cursor: pointer; }
  .settings-page { display: flex; flex-direction: column; gap: var(--card-gap); }
  .settings-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--card-gap); }
  @media (max-width: 900px) {
    .settings-grid { grid-template-columns: minmax(0, 1fr); }
  }
</style>
