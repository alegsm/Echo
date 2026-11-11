import type { EchoErrorCode } from '#lib/core/types.ts';
import { isNativeCaptureError } from '#lib/runtime/desktop/native-capture-client.ts';
import { isSttJobError } from '#lib/runtime/desktop/native-transcription-client.ts';

export function runtimeErrorCode(error: unknown): EchoErrorCode {
  if (hasEchoErrorCode(error)) return error.code;
  if (isNativeCaptureError(error)) {
    if (error.code === 'microphone_permission_denied') return 'microphone_permission_denied';
    // Matches capture_error in src-tauri/src/services/runtime.rs, which classifies capture events.
    if (error.code === 'microphone_unavailable' || error.code === 'device_unavailable') return 'microphone_unavailable';
    if (error.code === 'recording_too_short' || error.code === 'low_signal') return 'empty_audio';
    return 'capture_failed';
  }
  if (isSttJobError(error)) {
    if (error.code === 'missing_provider_key' || error.code === 'provider_credential_rejected') return 'missing_provider_key';
    if (error.code === 'provider_timeout') return 'provider_timeout';
    if (error.code === 'history_write_failed') return 'history_write_failed';
  }
  return 'provider_failed';
}

export function runtimeErrorMessage(error: unknown, code: EchoErrorCode) {
  if (isSttJobError(error)) return error.message;
  if (code === 'empty_audio') return 'No usable speech was captured.';
  if (code === 'microphone_permission_denied') return 'Microphone permission was denied. Open Settings to allow access.';
  if (code === 'microphone_unavailable') {
    if (isNativeCaptureError(error) && error.stage === 'permission_unconfirmed') return 'Microphone access could not be confirmed. Open Settings to check access.';
    return 'Microphone is unavailable.';
  }
  if (isNativeCaptureError(error) && error.code === 'microphone_busy') {
    return 'Dictation is using the microphone.';
  }
  return error instanceof Error ? error.message : 'Desktop runtime operation failed.';
}

function hasEchoErrorCode(error: unknown): error is { code: EchoErrorCode } {
  if (!error || typeof error !== 'object' || !('code' in error)) return false;
  return ['clipboard_failed', 'insertion_failed', 'history_write_failed', 'provider_failed']
    .includes(String(error.code));
}
