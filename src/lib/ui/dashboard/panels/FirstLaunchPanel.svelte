<script module lang="ts">
  export type FirstLaunchInitialState = { step?: number };
</script>

<script lang="ts">
  import { dashboardGlow } from '../dashboard-motion';
  import { tick, untrack } from 'svelte';
  import EchoMark from '#lib/ui/brand/EchoMark.svelte';
  import { getDashboardContext } from '../context.js';
  import { createSetupController } from './setup-controller';
  import { desktopPlatform } from './permissions-controller';
  import PermissionsPanel from './PermissionsPanel.svelte';
  import ProviderPanel from './ProviderPanel.svelte';

  let { initialState = {} }: { initialState?: FirstLaunchInitialState } = $props();
  const initial = untrack(() => initialState);
  const ctx = getDashboardContext();
  let permissionIntro = $derived(desktopPlatform(ctx.permissions()) === 'windows'
    ? 'Give Echo access to your microphone.' : 'Give Echo access to your microphone and recording shortcut.');
  const viewState = $state({ step: initial.step ?? 1 });
  const { setStep, finishSetup } = createSetupController(ctx, viewState, {
    tick, focusHeading: () => document.querySelector<HTMLElement>('.first-launch h1')?.focus()
  });
</script>

<div class="first-launch">
  <header>
    <EchoMark />
    <h1 tabindex="-1">{viewState.step === 1 ? 'Welcome to Echo' : 'Provider and key'}</h1>
    <p>{viewState.step === 1 ? permissionIntro : 'Choose a provider and add its key to start transcribing.'}</p>
  </header>
  <ol aria-label="Setup steps">
    {#each ['Permissions', 'Provider and key'] as label, i}
      <li {@attach dashboardGlow} class:echo-lit={viewState.step === i + 1} aria-current={viewState.step === i + 1 ? 'step' : undefined}>{i + 1}<span>{label}</span></li>
    {/each}
  </ol>
  {#if viewState.step === 1}
    <PermissionsPanel />
    <p class="hint">You can continue now and grant permissions later in Settings.</p>
    <footer><button type="button" onclick={() => ctx.setActiveTab('history')}>Browse Echo</button><button type="button" class="primary" onclick={() => setStep(2)}>Continue</button></footer>
  {:else}
    <ProviderPanel />
    <footer><button type="button" onclick={() => setStep(1)}>Back</button><button type="button" class="primary" onclick={finishSetup}>Start using Echo</button></footer>
  {/if}
</div>

<style>
  .first-launch { width: min(100%, 680px); margin: 0 auto; display: flex; flex-direction: column; gap: var(--card-gap); }
  header { display: flex; flex-direction: column; align-items: center; gap: 12px; padding: 8px 0 12px; text-align: center; text-shadow: var(--echo-field-halo); }
  h1 { margin: 0; font: var(--t-title); color: var(--echo-fg); }
  p { margin: 0; font: var(--t-body); color: var(--echo-fg-2); }
  ol { display: flex; flex-wrap: wrap; gap: 12px; list-style: none; margin: 0 0 6px; padding: 0; }
  li { display: flex; align-items: center; gap: 10px; flex: 1; padding: 12px 16px; border-radius: var(--r-control); background: var(--echo-well); color: var(--echo-fg-3); font: var(--t-meta); }
  li[aria-current] { color: var(--echo-fg); }
  .hint { font: var(--t-meta); }
  footer { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 12px; }
  button { min-height: 38px; padding: 0 18px; border: 0; border-radius: var(--r-bar); background: var(--echo-fill-1); color: var(--echo-fg); font: var(--t-label); cursor: pointer; box-shadow: inset 0 0 0 1px var(--echo-hairline); }
  button:hover { background: var(--echo-fill-2); }
  .primary { background: var(--echo-light); color: var(--echo-ink-on-light); }
  .primary:hover { background: var(--tone-record); }
</style>
