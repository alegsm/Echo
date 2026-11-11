<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { getDashboardContext } from '../context.js';
  import { attachTauriFileLogging, detachTauriFileLogging } from '#lib/runtime/tauri-log.ts';
  import { getDiagnosticStatus, subscribeDiagnosticStatus } from '#lib/runtime/debug-logging.ts';
  import { createDebugLogController, type DebugLogState } from './debug-log-controller';
  import DashboardCard from '../layout/DashboardCard.svelte';

  const ctx = getDashboardContext();
  const viewState = $state<DebugLogState>({ diagnosticStatus: getDiagnosticStatus(), logPath: undefined,
    logPathState: 'loading', actionMessage: '', isSaving: false });
  const { saveDiagnosticsSettings, mount } = createDebugLogController(ctx, viewState, {
    loadContext: () => invoke<{ logFilePath: string }>('get_diagnostic_context'),
    subscribeStatus: subscribeDiagnosticStatus, attach: attachTauriFileLogging, detach: detachTauriFileLogging
  });
  onMount(mount);
</script>

<section aria-labelledby="settings-debug">
  <DashboardCard padding="var(--card-pad)">
    <h2 id="settings-debug" tabindex="-1">Debug log</h2>
    <div class="debug-row">
      <div class="log-copy"><span id="debug-log-label">Detailed logging</span><div aria-live="polite">{#if viewState.logPathState === 'ready'}<code>{viewState.logPath}</code>{:else}<span class="path-message">{viewState.logPathState === 'loading' ? 'Loading log path…' : 'Log path unavailable'}</span>{/if}</div></div>
      <button type="button" class="echo-switch" role="switch" aria-checked={ctx.settings().debugLoggingEnabled} aria-labelledby="debug-log-label" disabled={viewState.isSaving} onclick={() => saveDiagnosticsSettings({ debugLoggingEnabled: !ctx.settings().debugLoggingEnabled })}></button>
    </div>
    {#if viewState.actionMessage}<p role="alert">{viewState.actionMessage}</p>{/if}
    {#if ctx.settings().debugLoggingEnabled && viewState.diagnosticStatus.state === 'error'}<p role="alert">{viewState.diagnosticStatus.message}</p>{/if}
  </DashboardCard>
</section>

<style>
  section { grid-column: 1 / -1; min-width: 0; }
  h2 { margin: 0 0 12px; font: var(--t-caps); letter-spacing: 0.08em; text-transform: uppercase; color: var(--echo-fg-3); }
  .debug-row { display: flex; align-items: center; justify-content: space-between; gap: 16px; }
  .log-copy { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
  .log-copy > span { font: var(--t-label); color: var(--echo-fg); }
  .path-message { font: var(--t-body); color: var(--echo-fg-3); }
  code { font: 400 11.5px/1.45 var(--mono); color: var(--echo-fg-3); overflow-wrap: anywhere; }
  p { margin: 10px 0 0; font: var(--t-meta); color: var(--tone-error-text); }
</style>
