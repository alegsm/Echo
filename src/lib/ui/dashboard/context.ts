import { getContext, setContext } from 'svelte';
import type { TranscriptRecord } from '#lib/core/transcript.ts';
import type { EchoRuntime, EchoSnapshot, DesktopPermissions } from '#lib/core/types.ts';
import type { EchoSettings, ProviderId } from '#lib/core/settings.ts';
export type { PermissionKind, PermissionItem, DesktopPermissions } from '#lib/core/types.ts';

export type SettingsTab = 'history' | 'phrasebook' | 'settings' | 'setup' | 'permissions';

const KEY = Symbol('echo:dashboard');

export type DashboardContext = {
  runtime: () => EchoRuntime | undefined;
  snapshot: () => EchoSnapshot;
  settings: () => EchoSettings;
  history: () => TranscriptRecord[];
  permissions: () => DesktopPermissions | undefined;
  permissionCheckState: () => 'checking' | 'ready' | 'unknown';
  loadHistory: (limit?: number) => Promise<void>;
  refreshPermissions: () => Promise<void>;
  setActionError: (message: string | null) => void;

  // Settings round-trip — panels call this instead of runtime.updateSettings
  // directly so the shell's reactive $state is always updated.
  updateSettings: (patch: Partial<EchoSettings>) => Promise<EchoSettings>;

  // Permissions immediate update — avoids the 1.5 s stale window after invoke.
  setPermissions: (next: DesktopPermissions) => void;

  // The rail permission status opens Settings and focuses Permissions.
  setActiveTab: (tab: SettingsTab) => void;

  // Native presence reports saved-key markers without reading the credential store.
  providerKeyPresence: () => Record<ProviderId, boolean>;
  refreshProviderKeyPresence: () => Promise<void>;
  setupRequired: () => boolean;

  // Drafts — shell-owned, panel-mutated. Lifted here so they survive tab
  // switches (panels unmount/remount; shell stays alive).
  apiKeyDrafts: () => Record<ProviderId, string>;
  setApiKeyDraft: (provider: ProviderId, value: string) => void;
  hotkeyDraft: () => string;
  setHotkeyDraft: (value: string) => void;
  phrasebookInput: () => string;
  setPhrasebookInput: (value: string) => void;
};

export function provideDashboardContext(ctx: DashboardContext): void {
  setContext(KEY, ctx);
}

export function getDashboardContext(): DashboardContext {
  const ctx = getContext<DashboardContext>(KEY);
  if (!ctx) throw new Error('Dashboard context missing — must be inside DashboardApp');
  return ctx;
}
