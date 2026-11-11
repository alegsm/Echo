<script module lang="ts">
  import type { DashboardSeed } from './dashboard-controller';
  import type { HistoryInitialState } from './panels/HistoryPanel.svelte';
  import type { FirstLaunchInitialState } from './panels/FirstLaunchPanel.svelte';
  import type { PhrasebookInitialState } from './panels/PhrasebookPanel.svelte';

  export type DashboardInitialState = DashboardSeed & {
    historyPanel?: HistoryInitialState;
    setup?: FirstLaunchInitialState;
    phrasebook?: PhrasebookInitialState;
  };
</script>

<script lang="ts">
  import { onMount, onDestroy, tick, untrack } from 'svelte';
  import './dashboard-app.css';
  import './dashboard-tokens.css';
  import { observeDashboardMotion } from './dashboard-motion';
  import ParticleField from './ParticleField.svelte';
  import { createEchoRuntime } from '#lib/runtime/runtime.ts';
  import { provideDashboardContext } from './context.js';
  import { createDashboardController, initialDashboardState } from './dashboard-controller';
  import DashboardNav from './nav/DashboardNav.svelte';
  import TitleBar from './TitleBar.svelte';
  import HistoryPanel from './panels/HistoryPanel.svelte';
  import SettingsPanel from './panels/SettingsPanel.svelte';
  import PhrasebookPanel from './panels/PhrasebookPanel.svelte';
  import FirstLaunchPanel from './panels/FirstLaunchPanel.svelte';
  import AlertBanner from './AlertBanner.svelte';

  let { initialState = {} }: { initialState?: DashboardInitialState } = $props();
  let motionAllowed = $state(false);
  onMount(() => observeDashboardMotion({ window, document }, (motion) => { motionAllowed = motion.active; }));
  const initial = untrack(() => initialState);
  const html = typeof document !== 'undefined' ? document.documentElement : null;
  // Component-owned proxies keep controller writes reactive across panel remounts.
  const viewState = $state(initialDashboardState(initial));
  const controller = createDashboardController(viewState, {
    tick, focusPermissions: () => {
      const heading = document.getElementById('settings-permissions');
      heading?.focus({ preventScroll: true });
      heading?.scrollIntoView({ block: 'start' });
    }
  });
  const { selectTab } = controller;
  const alert = $derived(controller.alert());
  provideDashboardContext(controller.context);
  onMount(() => controller.mount(() => createEchoRuntime({ hotkeyOwner: false }), html));
  onDestroy(() => {
    html?.removeAttribute('data-theme');
    html?.removeAttribute('data-accent');
  });
</script>

<svelte:head>
  <title>Echo Dashboard</title>
</svelte:head>

<ParticleField />
<div class="dashboard-shell" data-motion-paused={!motionAllowed}>
  <div class="dashboard-main">
    <TitleBar />
    <DashboardNav {motionAllowed} activeTab={viewState.settingsTab} onselect={selectTab} />
    <main class="dashboard-scroller">
      <div class="dashboard-column">
        <div aria-live="polite" aria-atomic="true" class="action-error-live">
          {#if alert}
            <AlertBanner label={alert.label} message={alert.message} />
          {/if}
        </div>
        {#if viewState.settingsTab === 'setup'}
          <FirstLaunchPanel initialState={initial.setup} />
        {:else if viewState.settingsTab === 'history'}
          <HistoryPanel initialState={initial.historyPanel} />
        {:else if viewState.settingsTab === 'phrasebook'}
          <PhrasebookPanel initialState={initial.phrasebook} />
        {:else}
          <SettingsPanel />
        {/if}
      </div>
    </main>
  </div>
</div>
