<script lang="ts">
  import { untrack } from 'svelte';
  import DashboardApp, { type DashboardInitialState } from '../../src/lib/ui/dashboard/DashboardApp.svelte';
  import PhrasebookPanel, { type PhrasebookInitialState } from '../../src/lib/ui/dashboard/panels/PhrasebookPanel.svelte';
  import KeybindsPanel from '../../src/lib/ui/dashboard/panels/KeybindsPanel.svelte';
  import type { EchoSnapshot } from '../../src/lib/core/types';
  import FnKeyPreferenceControl, { type FnKeyInitialState } from '../../src/lib/ui/dashboard/panels/FnKeyPreferenceControl.svelte';
  import { defaultSettings, providerOptions, type ProviderId } from '../../src/lib/core/settings';
  import { provideDashboardContext } from '../../src/lib/ui/dashboard/context';

  let { component = 'app', initialState = {}, phrasebook, fnKey, snapshot }: {
    component?: 'app' | 'phrasebook' | 'fn-key' | 'keybinds';
    initialState?: DashboardInitialState;
    phrasebook?: PhrasebookInitialState;
    fnKey?: FnKeyInitialState;
    snapshot?: Partial<EchoSnapshot>;
  } = $props();

  const initial = untrack(() => initialState);
  const settings = { ...defaultSettings, ...initial.settings };
  const drafts = Object.fromEntries(providerOptions.map(({ id }) => [id, ''])) as Record<ProviderId, string>;
  let hotkey = settings.pushToTalkShortcut;
  let phrase = '';
  provideDashboardContext({
    runtime: () => undefined,
    snapshot: () => ({ mode: 'desktop', state: 'idle', activeProviderId: settings.providerId, activeShortcut: hotkey, ...snapshot }),
    settings: () => settings, history: () => initial.history ?? [],
    permissions: () => initial.permissions,
    permissionCheckState: () => initial.permissionCheckState ?? 'checking',
    loadHistory: async () => { throw new Error('Render fixtures cannot load history'); },
    refreshPermissions: async () => { throw new Error('Render fixtures cannot check permissions'); },
    updateSettings: async () => { throw new Error('Render fixtures cannot save settings'); },
    setActionError() {}, setPermissions() {}, setActiveTab() {},
    providerKeyPresence: () => Object.fromEntries(providerOptions.map(({ id }) => [id, initial.providerKeyPresence?.[id] ?? false])) as Record<ProviderId, boolean>,
    refreshProviderKeyPresence: async () => { throw new Error('Render fixtures cannot check credentials'); },
    setupRequired: () => false,
    apiKeyDrafts: () => drafts, setApiKeyDraft: (id, value) => { drafts[id] = value; },
    hotkeyDraft: () => hotkey, setHotkeyDraft: (value) => { hotkey = value; },
    phrasebookInput: () => phrase, setPhrasebookInput: (value) => { phrase = value; }
  });
</script>

{#if component === 'phrasebook'}
  <PhrasebookPanel initialState={phrasebook} />
{:else if component === 'keybinds'}
  <KeybindsPanel />
{:else if component === 'fn-key'}
  <FnKeyPreferenceControl initialState={fnKey} />
{:else}
  <DashboardApp {initialState} />
{/if}
