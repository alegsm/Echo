import type { EchoSettings } from '#lib/core/settings.ts';
import type {
  NativeStorageClient,
  PersistedSettings
} from '#lib/runtime/desktop/native-storage-client.ts';

/** Keeps one window aligned to committed native settings revisions. */
export function createNativeSettingsStore(client: NativeStorageClient) {
  return new NativeSettingsStore(client);
}

class NativeSettingsStore {
  private current: PersistedSettings | undefined;
  private isDisposed = false;
  private readonly listeners = new Set<(settings: EchoSettings) => void>();
  private unlisten: (() => void) | undefined;

  constructor(private readonly client: NativeStorageClient) {}

  async start() {
    if (this.isDisposed || this.unlisten) return;
    const pendingUnlisten = await this.client.listenSettings((event) => {
      if (!this.current || event.revision > this.current.revision) void this.reload();
    });
    if (this.isDisposed) {
      pendingUnlisten();
      return;
    }
    this.unlisten = pendingUnlisten;
    await this.reload();
  }

  async reload() {
    const loaded = await this.client.getSettings();
    this.apply(loaded);
    return this.current?.settings;
  }

  async update(patch: Partial<EchoSettings>) {
    if (!this.current) await this.reload();
    const expectedRevision = this.current?.revision ?? 0;
    try {
      const result = await this.client.updateSettings(expectedRevision, patch);
      this.apply(result.current);
      return result.current.settings;
    } catch (error) {
      await this.reload();
      throw error;
    }
  }

  getRevision() {
    return this.current?.revision ?? 0;
  }

  get() {
    return this.current?.settings;
  }

  subscribe(listener: (settings: EchoSettings) => void) {
    if (this.isDisposed) return () => undefined;
    this.listeners.add(listener);
    if (this.current) listener(this.current.settings);
    return () => this.listeners.delete(listener);
  }

  dispose() {
    if (this.isDisposed) return;
    this.isDisposed = true;
    this.unlisten?.();
    this.unlisten = undefined;
    this.listeners.clear();
  }

  private apply(next: PersistedSettings) {
    if (this.isDisposed || (this.current && next.revision <= this.current.revision)) return;
    this.current = next;
    for (const listener of this.listeners) listener(next.settings);
  }
}
