import { invoke } from '@tauri-apps/api/core';
import {
  createDiagnosticEventWriter,
  type DiagnosticEvent,
  type DiagnosticStatus
} from '#lib/runtime/diagnostic-event-writer.ts';

export type { DiagnosticEvent, DiagnosticLevel, DiagnosticScalar, DiagnosticStatus } from '#lib/runtime/diagnostic-event-writer.ts';

const statusListeners = new Set<(status: DiagnosticStatus) => void>();
let status: DiagnosticStatus = { state: 'off', message: 'Detailed logging is off.' };
let hasLogProbeSucceeded = false;
let nativeSettingsOverride: boolean | undefined;

const diagnosticWriter = createDiagnosticEventWriter({
  isEnabled: debugLoggingEnabled,
  sink: (request) => invoke<boolean>('record_diagnostic_event', { request }),
  onStatus: setDiagnosticStatus
});

export function debugLoggingEnabled(): boolean {
  return nativeSettingsOverride ?? false;
}

/** Uses committed native settings as the desktop logging authority. */
export function setNativeDebugLoggingEnabled(enabled: boolean | undefined) {
  nativeSettingsOverride = enabled;
  if (!enabled) setDiagnosticStatus({ state: 'off', message: 'Detailed logging is off.' });
}

/** Writes one privacy-filtered diagnostic event when detailed logging is enabled. */
export async function recordDiagnosticEvent(event: DiagnosticEvent): Promise<boolean> {
  return diagnosticWriter.write(event);
}

export function getDiagnosticStatus(): DiagnosticStatus {
  return status;
}

export function subscribeDiagnosticStatus(listener: (next: DiagnosticStatus) => void): () => void {
  statusListeners.add(listener);
  listener(status);
  return () => statusListeners.delete(listener);
}

export function reportDiagnosticLogProbeFailure() {
  hasLogProbeSucceeded = false;
  setDiagnosticStatus(debugLoggingEnabled()
    ? { state: 'error', message: 'Could not verify write access to the diagnostic log folder.' }
    : { state: 'off', message: 'Detailed logging is off.' });
}

export function reportDiagnosticLogProbeSuccess() {
  hasLogProbeSucceeded = debugLoggingEnabled();
  if (!hasLogProbeSucceeded) {
    setDiagnosticStatus({ state: 'off', message: 'Detailed logging is off.' });
  }
}

function setDiagnosticStatus(next: DiagnosticStatus) {
  if (next.state === 'off') hasLogProbeSucceeded = false;
  if (next.state === 'ready' && !hasLogProbeSucceeded) return;
  if (status.state === next.state && status.message === next.message) return;
  status = next;
  for (const listener of statusListeners) listener(status);
}
