import type { EchoSettings, ProviderId } from '#lib/core/settings.ts';
import type {
  DesktopPermissions,
  EchoErrorCode,
  EchoRuntime,
  EchoRuntimeEvent,
  EchoSnapshot,
  PermissionKind,
  ProviderKeySearchOutcome
} from '#lib/core/types.ts';
import { createCaptureIntentController } from '#lib/runtime/desktop/capture-intent-controller.ts';
import { runtimeErrorCode, runtimeErrorMessage } from '#lib/runtime/desktop/desktop-error.ts';
import { createDictationController } from '#lib/runtime/desktop/dictation-controller.ts';
import {
  createHotkeyController,
  HotkeyConfigurationError,
  type HotkeyBoundary,
  type HotkeyClock
} from '#lib/runtime/desktop/hotkey-controller.ts';
import {
  createNativeCaptureClient,
  type CaptureEvent,
  type NativeCaptureBoundary
} from '#lib/runtime/desktop/native-capture-client.ts';
import { createNativeHistoryStore } from '#lib/runtime/desktop/native-history-store.ts';
import { createNativeRuntimeState } from '#lib/runtime/desktop/native-runtime-state.ts';
import { createNativeSettingsStore } from '#lib/runtime/desktop/native-settings-store.ts';
import { createNativeStorageClient } from '#lib/runtime/desktop/native-storage-client.ts';
import { createNativeTranscriptionClient } from '#lib/runtime/desktop/native-transcription-client.ts';
import { createPartialTranscriptState } from '#lib/runtime/desktop/partial-transcript-state.ts';
import { createDesktopTranscriptDelivery } from '#lib/runtime/desktop/transcript-delivery.ts';
import { createReportedTranscriptDelivery } from '#lib/runtime/desktop/reported-transcript-delivery.ts';
import { setNativeDebugLoggingEnabled } from '#lib/runtime/debug-logging.ts';

type DesktopRuntimeOptions = {
  boundary: NativeCaptureBoundary;
  hotkeyBoundary: HotkeyBoundary;
  hotkeyClock?: HotkeyClock;
  hotkeyOwner: boolean;
  syncDiagnostics(enabled: boolean): Promise<void>;
};

/** Composes the disposable metadata-only desktop runtime over native authority. */
export function createDesktopRuntime(options: DesktopRuntimeOptions): EchoRuntime {
  return new DesktopRuntime(options);
}

class DesktopRuntime implements EchoRuntime {
  readonly kind = 'tauri' as const;
  readonly mode = 'desktop' as const;
  private readonly dictation;
  private readonly history;
  private readonly hotkeys;
  private isDisposed = false;
  private readonly listeners = new Set<(event: EchoRuntimeEvent) => void>();
  private readonly ready: Promise<void>;
  private readonly partialTranscript = createPartialTranscriptState((projection) => {
    if (this.options.hotkeyOwner) this.emit({ type: 'partial_transcript_changed', projection });
  });
  private readonly settings;
  private readonly state;
  private readonly transcriptDelivery;
  private unlistenCredentials: (() => void) | undefined;

  constructor(private readonly options: DesktopRuntimeOptions) {
    const captureClient = createNativeCaptureClient(options.boundary);
    const storageClient = createNativeStorageClient(options.boundary);
    this.settings = createNativeSettingsStore(storageClient);
    this.state = createNativeRuntimeState(options.boundary);
    this.history = createNativeHistoryStore({
      client: storageClient,
      onChanged: (event) => this.emit({ type: 'history_changed', recordId: event.recordId })
    });
    this.transcriptDelivery = createDesktopTranscriptDelivery({
      boundary: options.boundary
    });
    const transcription = createNativeTranscriptionClient(options.boundary);
    const delivery = createReportedTranscriptDelivery({
      client: transcription,
      delivery: this.transcriptDelivery
    });
    this.dictation = createDictationController({
      capture: createCaptureIntentController({
        client: captureClient,
        purpose: 'push_to_talk',
        onIntent: (id) => this.partialTranscript.begin(id)
      }),
      delivery,
      onPersistenceError: () => this.emitError(
        'history_write_failed',
        'Transcript completed, but history could not be saved.'
      ),
      onTranscript: (transcript, dictationId) => this.emit({ type: 'transcript_completed', transcript, dictationId }),
      state: this.state,
      transcription
    });
    this.hotkeys = createHotkeyController({
      boundary: options.hotkeyBoundary,
      onError: (error) => this.emitShortcutError(error),
      clock: options.hotkeyClock,
      onCancel: async () => { await this.cancelRecording(); },
      onHandsFreeChanged: (active) => {
        if (options.hotkeyOwner) this.emit({ type: 'hands_free_changed', active });
      },
      onPress: async () => (await this.startRecording()).state === 'recording',
      onRelease: async () => { await this.stopRecording(); }
    });
    this.state.subscribe((snapshot) => {
      this.emit({ type: 'state_changed', snapshot });
    });
    this.settings.subscribe((settings) => {
      void this.applySettings(settings);
      this.emit({ type: 'settings_changed' });
    });
    this.ready = this.initialize();
  }

  async startRecording() {
    await this.ready;
    return this.settle(() => this.dictation.start());
  }

  async stopRecording() {
    if (this.hotkeys.hasIntent()) this.hotkeys.reset();
    await this.ready;
    return this.settle(() => this.dictation.stop());
  }

  async cancelRecording() {
    if (this.hotkeys.hasIntent()) this.hotkeys.reset();
    await this.ready;
    this.partialTranscript.clear();
    return this.settle(() => this.dictation.cancel());
  }

  async copyTranscript(id: string) {
    await this.ready;
    const transcript = await this.history.findRecord(id);
    if (!transcript) return;
    await this.transcriptDelivery.copy(transcript);
  }

  async listHistory(limit: number) {
    await this.ready;
    return this.history.list(limit);
  }

  async getHistoryCount() {
    await this.ready;
    return this.history.getTotalCount();
  }

  async deleteTranscript(id: string) {
    await this.ready;
    return this.history.delete(id);
  }

  async clearHistory() {
    await this.ready;
    return this.history.clear();
  }

  async previewHistoryRetention(retentionDays: number | undefined) {
    await this.ready;
    const preview = await this.history.previewRetention(retentionDays);
    return { deleteCount: preview.deleteCount, retainCount: preview.retainCount };
  }

  async getSettings() {
    await this.ready;
    const current = this.settings.get() ?? await this.settings.reload();
    if (!current) throw new Error('Desktop settings are unavailable.');
    return current;
  }

  async updateSettings(patch: Partial<EchoSettings>) {
    await this.ready;
    const expectedRevision = this.settings.getRevision();
    try {
      return await this.settings.update(patch);
    } catch (error) {
      if (Object.keys(patch).length !== 1 || !patch.providerKeyStatuses
        || this.settings.getRevision() <= expectedRevision) throw error;
      // The store reloads before rejecting. Retry status metadata once if settings advanced.
      return this.settings.update(patch);
    }
  }

  async getSnapshot() {
    await this.ready;
    return this.requireSnapshot();
  }

  subscribe(listener: (event: EchoRuntimeEvent) => void) {
    if (this.isDisposed) return () => undefined;
    this.listeners.add(listener);
    const snapshot = this.state.getSnapshot();
    if (snapshot) listener({ type: 'state_changed', snapshot });
    if (this.options.hotkeyOwner) listener({ type: 'hands_free_changed', active: this.hotkeys.isHandsFree() });
    const projection = this.partialTranscript.getProjection();
    if (this.options.hotkeyOwner && projection) listener({ type: 'partial_transcript_changed', projection });
    return () => this.listeners.delete(listener);
  }

  async getDesktopPermissions() {
    await this.ready;
    return this.options.boundary.invoke<DesktopPermissions>('get_desktop_permissions');
  }

  async requestDesktopPermission(kind: PermissionKind) {
    await this.ready;
    return this.options.boundary.invoke<DesktopPermissions>('request_desktop_permission', { kind });
  }

  async openDesktopPermissionSettings(kind: PermissionKind) {
    await this.ready;
    await this.options.boundary.invoke<void>('open_desktop_permission_settings', { kind });
  }

  async probeProviderKey(params: { providerId: ProviderId }) {
    await this.ready;
    return this.options.boundary.invoke<{
      ok: true;
      providerId: ProviderId;
      providerModel: string;
      timings: { backendMs: number; providerMs: number };
    }>('probe_provider', { request: { providerId: params.providerId } });
  }

  async setProviderApiKey(providerId: ProviderId, apiKey: string) {
    await this.ready;
    await this.options.boundary.invoke('set_provider_api_key', { providerId, apiKey });
    await this.settings.reload();
  }

  async deleteProviderApiKey(providerId: ProviderId) {
    await this.ready;
    await this.options.boundary.invoke('delete_provider_api_key', { providerId });
    await this.settings.reload();
  }

  async findProviderApiKey(providerId: ProviderId) {
    await this.ready;
    const { outcome } = await this.options.boundary.invoke<{ outcome: ProviderKeySearchOutcome }>(
      'find_provider_api_key', { providerId });
    // Only adoption commits a settings revision. A failed reload never discards the native outcome;
    // the committed-settings event reloads again.
    if (outcome === 'found') await this.settings.reload().catch(() => undefined);
    return outcome;
  }

  async getProviderApiKeyPresence() {
    await this.ready;
    return this.options.boundary.invoke<Record<ProviderId, boolean>>('provider_api_key_presence');
  }

  async dispose() {
    if (this.isDisposed) return;
    if (this.hotkeys.hasIntent()) this.hotkeys.reset();
    this.isDisposed = true;
    this.listeners.clear();
    this.unlistenCredentials?.();
    this.unlistenCredentials = undefined;
    this.settings.dispose();
    this.history.dispose();
    this.state.dispose();
    this.partialTranscript.dispose();
    setNativeDebugLoggingEnabled(undefined);
    await Promise.all([this.hotkeys.dispose(), this.dictation.dispose()]);
  }

  private async initialize() {
    await Promise.all([this.settings.start(), this.state.start(), this.history.start()]);
    if (this.isDisposed) return;
    if (this.options.hotkeyOwner) await this.partialTranscript.start(this.options.boundary, (event) => this.onCapture(event));
    if (this.isDisposed) return;
    await this.listenForCredentialChanges();
    if (this.options.hotkeyOwner) await this.hotkeys.start();
  }

  private onCapture(event: CaptureEvent) {
    if (!this.dictation.matchesCapture(event)) return;
    if (event.phase === 'recording') {
      this.emit({ type: 'recording_progress', intentId: event.intentId!, sessionId: event.sessionId!,
        elapsedMs: event.elapsedMs, limitMs: event.limitMs });
    }
    if (event.phase !== 'idle') return;
    if (event.outcome === 'artifact_created' && event.artifactId) {
      this.hotkeys.reset(true);
      void this.settle(() => this.dictation.stop(event));
    } else if (event.error) {
      this.hotkeys.reset();
      this.partialTranscript.clear(event.intentId!);
      void this.settle(() => this.dictation.cancel());
    }
  }

  private async applySettings(settings: EchoSettings) {
    setNativeDebugLoggingEnabled(settings.debugLoggingEnabled);
    await this.options.syncDiagnostics(settings.debugLoggingEnabled).catch(() => undefined);
    if (!this.options.hotkeyOwner) return;
    await this.hotkeys
      .configure(settings.pushToTalkShortcut, settings.fallbackShortcut)
      .catch((error: unknown) => this.emitShortcutError(error));
  }

  private async listenForCredentialChanges() {
    const pending = await this.options.boundary.listen('echo-credentials-updated', () => {
      this.emit({ type: 'credentials_updated' });
    });
    if (this.isDisposed) pending();
    else this.unlistenCredentials = pending;
  }

  private async settle(action: () => Promise<EchoSnapshot | undefined>) {
    let dictationId = this.partialTranscript.getProjection()?.dictationId;
    try {
      const pending = action();
      dictationId = this.partialTranscript.getProjection()?.dictationId;
      return (await pending) ?? this.requireSnapshot();
    } catch (error) {
      if (dictationId) this.partialTranscript.clear(dictationId);
      const code = runtimeErrorCode(error);
      await this.state.refresh().catch(() => undefined);
      this.emitError(code, runtimeErrorMessage(error, code));
      return this.requireSnapshot();
    }
  }

  private requireSnapshot() {
    const snapshot = this.state.getSnapshot();
    if (!snapshot) throw new Error('Desktop runtime snapshot is unavailable.');
    return snapshot;
  }

  private emit(event: EchoRuntimeEvent) {
    if (this.isDisposed) return;
    for (const listener of this.listeners) listener(event);
  }

  private emitShortcutError(error: unknown) {
    this.emitError('shortcut_conflict', error instanceof HotkeyConfigurationError
      ? error.message : 'Shortcut is already in use.');
  }

  private emitError(code: EchoErrorCode, message: string) {
    this.emit({ type: 'error', code, message });
  }

}
