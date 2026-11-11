import { expect, test } from 'bun:test';
import { runtimeErrorCode, runtimeErrorMessage } from '../src/lib/runtime/desktop/desktop-error';

// Merge: native code and stage select permission recovery without inventing denial.
// Contract: the classification matches capture_error in src-tauri/src/services/runtime.rs, so a
// rejected start and a capture event give the owner the same message for the same native code.
test.each([
  ['microphone_permission_denied', 'stream_build', 'microphone_permission_denied', 'Microphone permission was denied. Open Settings to allow access.'],
  ['microphone_unavailable', 'permission_unconfirmed', 'microphone_unavailable', 'Microphone access could not be confirmed. Open Settings to check access.'],
  ['microphone_unavailable', 'permission', 'microphone_unavailable', 'Microphone is unavailable.'],
  ['device_unavailable', 'default_config', 'microphone_unavailable', 'Microphone is unavailable.'],
  ['device_unavailable', 'first_callback', 'microphone_unavailable', 'Microphone is unavailable.']
] as const)('%s at %s becomes %s with its recovery message', (nativeCode, stage, code, message) => {
  const error = { code: nativeCode, phase: stage.startsWith('permission') ? 'permission_pending' : 'opening', retryable: true, stage };
  expect(runtimeErrorCode(error)).toBe(code);
  expect(runtimeErrorMessage(error, runtimeErrorCode(error))).toBe(message);
});
