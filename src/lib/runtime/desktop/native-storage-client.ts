import type { EchoSettings } from '#lib/core/settings.ts';
import type { TranscriptRecord } from '#lib/core/transcript.ts';
import type { NativeCaptureBoundary } from '#lib/runtime/desktop/native-capture-client.ts';

export type PersistedSettings = { revision: number; settings: EchoSettings };
type NativeSettingsPatch = Omit<Partial<EchoSettings>,
  'customPrompt' | 'fallbackShortcut' | 'historyRetentionDays' | 'language'
> & {
  customPrompt?: string | null;
  fallbackShortcut?: string | null;
  historyRetentionDays?: number | null;
  language?: string | null;
};
export type CleanupStatus = 'complete' | 'pending';

type SettingsCommit = {
  cleanupStatus: CleanupStatus;
  changedFields: string[];
  current: PersistedSettings;
  previous: PersistedSettings;
};
export type RevisedTranscript = { record: TranscriptRecord; revision: number };
export type HistoryCursor = { createdAt: string; id: string };
type HistoryPage = {
  cleanupStatus: CleanupStatus;
  nextCursor?: HistoryCursor;
  records: RevisedTranscript[];
  revision: number;
  totalCount: number;
};
type HistoryMutation = {
  cleanupStatus?: CleanupStatus;
  affectedCount: number;
  historyRevision: number;
  recordId?: string;
  transcript?: RevisedTranscript;
};
type HistoryRetentionPreview = {
  deleteCount: number;
  historyRevision: number;
  retainCount: number;
  retentionDays: number | null;
};
type SettingsCommittedEvent = { changedFields: string[]; revision: number };
export type HistoryCommittedEvent = {
  action: 'append' | 'update' | 'delete' | 'clear' | 'purge';
  affectedCount?: number;
  historyRevision: number;
  recordId?: string;
};

/** Typed access to revision-checked native settings and transcript history. */
export function createNativeStorageClient(boundary: NativeCaptureBoundary) {
  return {
    async getSettings() {
      return normalizeSettingsEnvelope(await boundary.invoke<PersistedSettings>('get_persisted_settings'));
    },
    async updateSettings(expectedRevision: number, patch: Partial<EchoSettings>) {
      const result = await boundary.invoke<SettingsCommit>('update_persisted_settings', {
        expectedRevision,
        patch: encodeSettingsPatch(patch)
      });
      return {
        ...result,
        current: normalizeSettingsEnvelope(result.current),
        previous: normalizeSettingsEnvelope(result.previous)
      };
    },
    listHistory: (limit: number, cursor?: HistoryCursor) =>
      boundary.invoke<HistoryPage>('list_persisted_history', { limit, cursor }),
    deleteTranscript: (id: string, expectedRecordRevision: number) =>
      boundary.invoke<HistoryMutation>('delete_persisted_transcript', {
        id,
        expectedRecordRevision
      }),
    clearHistory: (expectedHistoryRevision: number) =>
      boundary.invoke<HistoryMutation>('clear_persisted_history', { expectedHistoryRevision }),
    getTranscript: (id: string) =>
      boundary.invoke<RevisedTranscript | null>('get_persisted_transcript', { id }),
    previewRetention: (retentionDays: number | null) =>
      boundary.invoke<HistoryRetentionPreview>('preview_history_retention', { retentionDays }),
    listenSettings: (listener: (event: SettingsCommittedEvent) => void) =>
      boundary.listen<SettingsCommittedEvent>('echo-settings-committed', (event) => listener(event.payload)),
    listenHistory: (listener: (event: HistoryCommittedEvent) => void) =>
      boundary.listen<HistoryCommittedEvent>('echo-history-committed', (event) => listener(event.payload))
  };
}

export type NativeStorageClient = ReturnType<typeof createNativeStorageClient>;

function encodeSettingsPatch(patch: Partial<EchoSettings>): NativeSettingsPatch {
  const result: NativeSettingsPatch = { ...patch };
  for (const field of ['fallbackShortcut', 'language', 'customPrompt', 'historyRetentionDays'] as const) {
    if (field in patch && patch[field] === undefined) result[field] = null;
  }
  return result;
}

function normalizeSettingsEnvelope(envelope: PersistedSettings): PersistedSettings {
  const settings = { ...envelope.settings } as EchoSettings & Record<string, unknown>;
  for (const field of ['fallbackShortcut', 'language', 'customPrompt', 'historyRetentionDays'] as const) {
    if (settings[field] === null) settings[field] = undefined;
  }
  return { ...envelope, settings };
}
