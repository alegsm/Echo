import type { TranscriptRecord } from '#lib/core/transcript.ts';
import type {
  HistoryCursor,
  HistoryCommittedEvent,
  NativeStorageClient,
  RevisedTranscript
} from '#lib/runtime/desktop/native-storage-client.ts';

const MAX_NATIVE_HISTORY_PAGE = 200;

type NativeHistoryStoreOptions = {
  client: NativeStorageClient;
  onChanged(event: HistoryCommittedEvent): void;
};

/** Resolves native record revisions for history actions without duplicating history authority. */
export function createNativeHistoryStore(options: NativeHistoryStoreOptions) {
  return new NativeHistoryStore(options);
}

class NativeHistoryStore {
  private isDisposed = false;
  private historyRevision: number | undefined;
  private readonly records = new Map<string, RevisedTranscript>();
  private totalCount = 0;
  private unlisten: (() => void) | undefined;

  constructor(private readonly options: NativeHistoryStoreOptions) {}

  async start() {
    if (this.isDisposed || this.unlisten) return;
    const pendingUnlisten = await this.options.client.listenHistory((event) => {
      this.records.clear();
      this.historyRevision = event.historyRevision;
      if (!this.isDisposed) this.options.onChanged(event);
    });
    if (this.isDisposed) pendingUnlisten();
    else this.unlisten = pendingUnlisten;
  }

  async list(limit: number) {
    const requestedLimit = Math.max(0, Math.floor(limit));
    if (requestedLimit === 0) return [];
    const records = await this.loadPages(requestedLimit);
    this.remember(records);
    return records.map((entry) => entry.record);
  }

  async findRecord(id: string) {
    return (await this.find(id))?.record;
  }

  async delete(id: string) {
    const current = await this.find(id);
    if (!current) return 0;
    try {
      const result = await this.options.client.deleteTranscript(id, current.revision);
      this.records.delete(id);
      this.historyRevision = result.historyRevision;
      this.totalCount = Math.max(0, this.totalCount - result.affectedCount);
      return result.affectedCount;
    } catch (error) {
      await this.reloadSummary();
      throw error;
    }
  }

  async clear() {
    if (this.historyRevision === undefined) await this.reloadSummary();
    try {
      const result = await this.options.client.clearHistory(this.historyRevision ?? 0);
      this.records.clear();
      this.historyRevision = result.historyRevision;
      this.totalCount = 0;
      return result.affectedCount;
    } catch (error) {
      await this.reloadSummary();
      throw error;
    }
  }

  async previewRetention(retentionDays: number | undefined) {
    return this.options.client.previewRetention(retentionDays ?? null);
  }

  getTotalCount() {
    return this.totalCount;
  }

  dispose() {
    if (this.isDisposed) return;
    this.isDisposed = true;
    this.unlisten?.();
    this.unlisten = undefined;
    this.records.clear();
  }

  private async find(id: string) {
    const known = this.records.get(id);
    if (known) return known;
    const loaded = await this.options.client.getTranscript(id);
    if (loaded) this.records.set(id, loaded);
    return loaded ?? undefined;
  }

  private remember(records: RevisedTranscript[]) {
    if (this.isDisposed) return;
    for (const record of records) this.records.set(record.record.id, record);
  }

  private async loadPages(limit: number, retry = 0): Promise<RevisedTranscript[]> {
    const records: RevisedTranscript[] = [];
    let cursor: HistoryCursor | undefined;
    let revision: number | undefined;
    while (records.length < limit) {
      const pageLimit = Math.min(MAX_NATIVE_HISTORY_PAGE, limit - records.length);
      const page = await this.options.client.listHistory(pageLimit, cursor);
      if (revision !== undefined && page.revision !== revision) {
        if (retry >= 1) throw new Error('History changed while loading. Refresh and try again.');
        return this.loadPages(limit, retry + 1);
      }
      revision = page.revision;
      this.historyRevision = page.revision;
      this.totalCount = page.totalCount;
      records.push(...page.records);
      cursor = page.nextCursor;
      if (!cursor || page.records.length === 0) break;
    }
    return records;
  }

  private async reloadSummary() {
    const page = await this.options.client.listHistory(1);
    this.historyRevision = page.revision;
    this.totalCount = page.totalCount;
    this.records.clear();
    this.remember(page.records);
  }
}
