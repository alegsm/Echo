import { defaultSettings, providerOptions, type EchoSettings, type ProviderId } from '#lib/core/settings.ts';
import type { EchoErrorCode, EchoRuntime, EchoRuntimeEvent, EchoSnapshot, DesktopPermissions } from '#lib/core/types.ts';
import type { TranscriptRecord } from '#lib/core/transcript.ts';
import type { DashboardContext, SettingsTab } from './context';

export type DashboardSeed = {
  tab?: SettingsTab;
  settings?: Partial<EchoSettings>;
  history?: TranscriptRecord[];
  permissions?: DesktopPermissions;
  permissionCheckState?: 'checking' | 'ready' | 'unknown';
  actionError?: string | null;
  providerKeyPresence?: Partial<Record<ProviderId, boolean>>;
  providerPresenceKnown?: boolean;
};

export type DashboardState = {
  runtime: EchoRuntime | undefined;
  snapshot: EchoSnapshot;
  settings: EchoSettings;
  settingsTab: SettingsTab;
  history: TranscriptRecord[];
  historyLimit: number;
  permissions: DesktopPermissions | undefined;
  permissionCheckState: 'checking' | 'ready' | 'unknown';
  bootError: string | null;
  actionError: string | null;
  apiKeyDrafts: Record<ProviderId, string>;
  hotkeyDraft: string;
  phrasebookInput: string;
  providerKeyPresence: Record<ProviderId, boolean>;
  providerPresenceKnown: boolean;
};

export function initialDashboardState(initial: DashboardSeed = {}): DashboardState {
  return {
    runtime: undefined,
    snapshot: { mode: 'desktop', state: 'idle', activeProviderId: 'groq-whisper-turbo', activeShortcut: defaultSettings.pushToTalkShortcut },
    settings: { ...defaultSettings, ...initial.settings }, settingsTab: initial.tab ?? 'history',
    history: initial.history ?? [], historyLimit: 100,
    permissions: initial.permissions, permissionCheckState: initial.permissionCheckState ?? 'checking',
    bootError: null, actionError: initial.actionError ?? null,
    apiKeyDrafts: Object.fromEntries(providerOptions.map((p) => [p.id, ''])) as Record<ProviderId, string>,
    hotkeyDraft: defaultSettings.pushToTalkShortcut, phrasebookInput: '',
    providerKeyPresence: Object.fromEntries(providerOptions.map((p) => [p.id, initial.providerKeyPresence?.[p.id] ?? false])) as Record<ProviderId, boolean>,
    providerPresenceKnown: initial.providerPresenceKnown ?? false
  };
}

export const errorLabels: Record<EchoErrorCode, string> = {
  shortcut_conflict: 'Shortcut is already in use.',
  microphone_permission_denied: 'Microphone permission was denied.',
  microphone_unavailable: 'No microphone is available.',
  capture_failed: 'Recording failed.',
  empty_audio: 'No speech was captured.',
  missing_provider_key: 'Provider key missing. Add a key or use Find saved key in Settings > Provider.',
  provider_timeout: 'Provider timed out.',
  provider_failed: 'Provider transcription failed.',
  clipboard_failed: 'Transcript could not be copied.',
  insertion_failed: 'Transcript could not be inserted.',
  history_write_failed: 'Transcript saved locally, but history write failed.',
  not_implemented: 'This runtime path is not implemented yet.'
};

type DashboardEffects = { tick: () => Promise<void>; focusPermissions: () => void };

export function createDashboardController(state: DashboardState, effects: DashboardEffects) {
  let setupEvaluated = false;
  let presenceRequest = 0;

  function alert(): { label: string; message: string } | null {
    if (state.bootError) return { label: 'Network Failure', message: state.bootError };
    if (state.actionError) return { label: 'Action failed', message: state.actionError };
    const code = state.snapshot.lastError;
    if (!code) return null;
    const label = code === 'missing_provider_key' ? 'Provider setup'
      : code === 'provider_timeout' || code === 'provider_failed' ? 'Network Failure'
      : 'Action failed';
    return { label, message: errorLabels[code] };
  }

  function selectTab(tab: SettingsTab) {
    state.settingsTab = tab === 'permissions' ? 'settings' : tab;
    if (tab === 'permissions') void focusPermissions();
  }

  async function focusPermissions() {
    await effects.tick();
    effects.focusPermissions();
  }

  function setActionError(message: string | null) {
    state.actionError = message;
  }

  async function updateSettings(patch: Partial<EchoSettings>): Promise<EchoSettings> {
    if (!state.runtime) throw new Error('Runtime not ready');
    state.settings = await state.runtime.updateSettings(patch);
    return state.settings;
  }

  function setPermissions(next: DesktopPermissions) {
    state.permissions = next;
    state.permissionCheckState = 'ready';
  }

  function needsSetup() {
    return state.providerPresenceKnown && !Object.values(state.providerKeyPresence).some(Boolean);
  }

  async function refreshProviderKeyPresence() {
    const request = ++presenceRequest;
    try {
      const presence = await state.runtime?.getProviderApiKeyPresence();
      if (presence && request === presenceRequest) {
        state.providerKeyPresence = presence;
        state.providerPresenceKnown = true;
        if (!setupEvaluated) {
          setupEvaluated = true;
          if (needsSetup() && state.settingsTab === 'history') state.settingsTab = 'setup';
        }
      }
    } catch {
      if (request === presenceRequest) state.actionError = 'Provider configuration could not be checked. Try again.';
    }
  }

  function handleRuntimeEvent(event: EchoRuntimeEvent) {
    if (event.type === 'state_changed') {
      state.snapshot = event.snapshot;
    }

    if (event.type === 'transcript_completed') {
      state.history = [event.transcript, ...state.history.filter((record) => record.id !== event.transcript.id)];
    }

    if (event.type === 'settings_changed') {
      void state.runtime?.getSettings().then((next) => { state.settings = next; void refreshProviderKeyPresence(); })
        .catch(() => { state.actionError = 'Settings could not be refreshed. Try again.'; });
    }

    if (event.type === 'credentials_updated') {
      void refreshProviderKeyPresence();
    }

    if (event.type === 'error') {
      state.actionError = event.message || errorLabels[event.code];
    }
  }

  async function refreshPermissions() {
    state.permissionCheckState = 'checking';
    try {
      state.permissions = await state.runtime?.getDesktopPermissions();
      state.permissionCheckState = state.permissions ? 'ready' : 'unknown';
    } catch {
      state.permissions = undefined;
      state.permissionCheckState = 'unknown';
      state.actionError = 'Permission check failed. Re-check to try again.';
    }
  }

  async function loadHistory(limit = state.historyLimit) {
    if (!state.runtime) return;

    state.historyLimit = Math.max(100, Math.floor(limit));
    state.history = await state.runtime.listHistory(state.historyLimit);
  }

  function setSettingsFromRuntime(nextSettings: EchoSettings) {
    if (state.hotkeyDraft === state.settings.pushToTalkShortcut) {
      state.hotkeyDraft = nextSettings.pushToTalkShortcut;
    }
    state.settings = nextSettings;
    void refreshProviderKeyPresence();
  }
  const context: DashboardContext = {
    runtime: () => state.runtime, snapshot: () => state.snapshot,
    settings: () => state.settings, history: () => state.history,
    permissions: () => state.permissions, permissionCheckState: () => state.permissionCheckState,
    loadHistory, refreshPermissions, setActionError, updateSettings, setPermissions,
    providerKeyPresence: () => state.providerKeyPresence, refreshProviderKeyPresence, setupRequired: needsSetup,
    apiKeyDrafts: () => state.apiKeyDrafts,
    setApiKeyDraft: (provider, value) => { state.apiKeyDrafts = { ...state.apiKeyDrafts, [provider]: value }; },
    hotkeyDraft: () => state.hotkeyDraft, setHotkeyDraft: (value) => { state.hotkeyDraft = value; },
    phrasebookInput: () => state.phrasebookInput, setPhrasebookInput: (value) => { state.phrasebookInput = value; },
    setActiveTab: selectTab
  };

  function mount(createRuntime: () => EchoRuntime, html?: Pick<HTMLElement, 'setAttribute'> | null) {
    html?.setAttribute('data-theme', 'chalk');
    html?.setAttribute('data-accent', 'chalk');
    state.runtime = createRuntime();
    const unsubscribe = state.runtime.subscribe(handleRuntimeEvent);
    state.runtime.getSnapshot().then((next) => { state.snapshot = next; }).catch((error: unknown) => {
      state.bootError = error instanceof Error ? error.message : 'Runtime failed to boot';
    });
    state.runtime.getSettings().then(setSettingsFromRuntime).catch((error: unknown) => {
      state.bootError = error instanceof Error ? error.message : 'Settings failed to load';
    });
    void refreshPermissions();
    return () => { unsubscribe(); void state.runtime?.dispose(); };
  }

  return { context, selectTab, handleRuntimeEvent, mount, alert };
}
