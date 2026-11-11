import type { EchoSettings } from './settings';
import type { ProviderId } from './settings';
import type { TranscriptRecord } from './transcript';

export type PermissionKind = 'microphone' | 'accessibility' | 'input_monitoring';
export type PermissionItem = {
  status: string;
  granted: boolean;
  message: string | undefined;
};
export type DesktopPermissions = Record<PermissionKind, PermissionItem> & { platform: 'macos' | 'windows' | 'linux' };

type EchoMode = 'desktop';

type EchoState =
  | 'idle'
  | 'requesting_microphone'
  | 'recording'
  | 'stopping'
  | 'transcribing'
  | 'inserting'
  | 'done'
  | 'error';

export type EchoErrorCode =
  | 'shortcut_conflict'
  | 'microphone_permission_denied'
  | 'microphone_unavailable'
  | 'capture_failed'
  | 'empty_audio'
  | 'missing_provider_key'
  | 'provider_timeout'
  | 'provider_failed'
  | 'clipboard_failed'
  | 'insertion_failed'
  | 'history_write_failed'
  | 'not_implemented';

export type EchoSnapshot = {
  nativeModifierAvailable?: boolean | null;
  mode: EchoMode;
  state: EchoState;
  activeProviderId: string;
  activeShortcut: string;
  lastError?: EchoErrorCode;
  lastTranscript?: TranscriptRecord;
};

/** Outcome of an explicit saved-key search; it never carries the key. */
export type ProviderKeySearchOutcome = 'found' | 'not_found' | 'invalid' | 'unavailable' | 'already_saved' | 'save_failed';

export type PartialTranscriptStatus = 'retrying' | 'gap' | 'recovered';

export type PartialTranscriptProjection = {
  status?: PartialTranscriptStatus;
  /** Piece the status describes; text ordering still follows `ordinal`. */
  statusOrdinal?: number;
  dictationId: string;
  ordinal: number;
  text: string;
  eligible: boolean;
  phase: 'active' | 'completed' | 'cleared';
};

export type EchoRuntimeEvent =
  | { type: 'state_changed'; snapshot: EchoSnapshot }
  | { type: 'hands_free_changed'; active: boolean }
  | { type: 'recording_progress'; intentId: string; sessionId: string; elapsedMs: number; limitMs: number }
  | { type: 'partial_transcript_changed'; projection: PartialTranscriptProjection }
  | { type: 'transcript_completed'; transcript: TranscriptRecord; dictationId?: string }
  | { type: 'history_changed'; recordId?: string }
  | { type: 'credentials_updated' }
  | { type: 'settings_changed' }
  | { type: 'error'; code: EchoErrorCode; message: string };

export interface EchoRuntime {
  mode: EchoMode;
  kind: 'tauri';
  startRecording(): Promise<EchoSnapshot>;
  stopRecording(): Promise<EchoSnapshot>;
  cancelRecording(): Promise<EchoSnapshot>;
  copyTranscript(id: string): Promise<void>;
  listHistory(limit: number): Promise<TranscriptRecord[]>;
  getHistoryCount(): Promise<number>;
  deleteTranscript(id: string): Promise<number>;
  clearHistory(): Promise<number>;
  previewHistoryRetention(retentionDays: number | undefined): Promise<{
    deleteCount: number;
    retainCount: number;
  }>;
  getSettings(): Promise<EchoSettings>;
  updateSettings(patch: Partial<EchoSettings>): Promise<EchoSettings>;
  getSnapshot(): Promise<EchoSnapshot>;
  subscribe(listener: (event: EchoRuntimeEvent) => void): () => void;
  getDesktopPermissions(): Promise<DesktopPermissions>;
  requestDesktopPermission(kind: PermissionKind): Promise<DesktopPermissions>;
  openDesktopPermissionSettings(kind: PermissionKind): Promise<void>;
  probeProviderKey(params: {
    providerId: ProviderId;
  }): Promise<{ ok: true; providerId: string; providerModel: string; timings: { backendMs: number } }>;
  setProviderApiKey(providerId: ProviderId, apiKey: string): Promise<void>;
  deleteProviderApiKey(providerId: ProviderId): Promise<void>;
  findProviderApiKey(providerId: ProviderId): Promise<ProviderKeySearchOutcome>;
  getProviderApiKeyPresence(): Promise<Record<ProviderId, boolean>>;
  dispose(): void | Promise<void>;
}
