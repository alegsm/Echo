import type { NativeCaptureBoundary } from '#lib/runtime/desktop/native-capture-client.ts';

/** `version` is semver without a leading v. `totalBytes` is null when the server sends no length. */
export type UpdateStatus =
  | { state: 'none' }
  | { state: 'available' | 'waiting_for_idle' | 'installing' | 'failed'; version: string }
  | { state: 'downloading'; version: string; downloadedBytes: number; totalBytes: number | null };

/** `sequence` rises by one on every native status change. */
export type UpdateEnvelope = { sequence: number; status: UpdateStatus };

/**
 * Dashboard-only access to the native updater. Rust owns every update request; neither command takes
 * a payload. `install` returns once the request is admitted or ignored, never after the download.
 */
export function createUpdateClient(boundary: NativeCaptureBoundary) {
  return {
    status: () => boundary.invoke<UpdateEnvelope>('get_update_status'),
    install: () => boundary.invoke<UpdateEnvelope>('install_update'),
    listen: (listener: (envelope: UpdateEnvelope) => void) =>
      boundary.listen<UpdateEnvelope>('echo-update-status', (event) => listener(event.payload))
  };
}

export type UpdateClient = ReturnType<typeof createUpdateClient>;
