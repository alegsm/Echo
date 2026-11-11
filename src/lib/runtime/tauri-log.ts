import { invoke } from '@tauri-apps/api/core';
import {
  debugLoggingEnabled,
  recordDiagnosticEvent,
  reportDiagnosticLogProbeFailure,
  reportDiagnosticLogProbeSuccess
} from '#lib/runtime/debug-logging.ts';

let toggleGeneration = 0;

export async function attachTauriFileLogging(): Promise<boolean> {
  if (typeof window === 'undefined' || !window.__TAURI_INTERNALS__) return false;
  if (!debugLoggingEnabled()) return false;
  const generation = ++toggleGeneration;
  try {
    await invoke('probe_diagnostic_log');
  } catch {
    if (isCurrent(true, generation)) reportDiagnosticLogProbeFailure();
    return false;
  }
  if (!isCurrent(true, generation)) return false;
  reportDiagnosticLogProbeSuccess();
  return recordDiagnosticEvent({ area: 'diagnostics', event: 'enabled' });
}

export async function detachTauriFileLogging(): Promise<void> {
  ++toggleGeneration;
  if (typeof window === 'undefined' || !window.__TAURI_INTERNALS__) return;
  // This event is intentionally suppressed after the setting is persisted as off.
  void recordDiagnosticEvent({ area: 'diagnostics', event: 'disabled' });
}

function isCurrent(enabled: boolean, generation: number) {
  return generation === toggleGeneration && debugLoggingEnabled() === enabled;
}
