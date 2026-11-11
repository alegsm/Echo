export type CapturePurpose = 'push_to_talk';
type CapturePhase =
  | 'idle'
  | 'permission_pending'
  | 'opening'
  | 'waiting_for_first_callback'
  | 'recording'
  | 'stopping';

type CaptureErrorCode =
  | 'microphone_busy'
  | 'microphone_permission_denied'
  | 'microphone_unavailable'
  | 'device_config_failed'
  | 'stream_build_failed'
  | 'stream_play_failed'
  | 'device_unavailable'
  | 'stream_invalidated'
  | 'buffer_underrun'
  | 'stream_backend_error'
  | 'first_callback_timeout'
  | 'recording_too_short'
  | 'low_signal'
  | 'pcm_memory_limit'
  | 'artifact_size_limit'
  | 'artifact_capacity_reached'
  | 'artifact_expired'
  | 'invalid_owner'
  | 'invalid_reservation'
  | 'session_not_found'
  | 'capture_cancelled'
  | 'coordinator_unavailable';

type NativeCaptureError = {
  activePurpose: CapturePurpose | null;
  code: CaptureErrorCode;
  phase: CapturePhase;
  retryable: boolean;
  stage: string;
};

export type CaptureSession = {
  channelCount: number;
  intentId: string;
  phase: 'recording';
  purpose: CapturePurpose;
  sampleFormat: string;
  sampleRate: number;
  sessionId: string;
};

export type AudioArtifact = {
  artifactId: string;
  byteSize: number;
  channelCount: number;
  durationMs: number;
  mimeType: 'audio/wav';
  purpose: CapturePurpose;
  sampleRate: number;
  sessionId: string;
};

export type CaptureEvent = {
  elapsedMs: number;
  limitMs: number;
  artifactBytes: number;
  artifactCount: number;
  artifactId: string | null;
  callbackCount: number;
  error: CaptureErrorCode | null;
  hasObservedCallback: boolean;
  intentId: string | null;
  lastError: CaptureErrorCode | null;
  outcome: string;
  phase: CapturePhase;
  purpose: CapturePurpose | null;
  sampleCount: number;
  sequence: number;
  sessionId: string | null;
};

export type NativeCaptureBoundary = {
  invoke<T>(command: string, argumentsValue?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, listener: (event: { payload: T }) => void): Promise<() => void>;
};

/** Typed renderer adapter for the metadata-only native audio coordinator. */
export function createNativeCaptureClient(boundary: NativeCaptureBoundary) {
  return {
    start: (purpose: CapturePurpose, intentId: string) =>
      boundary.invoke<CaptureSession>('start_audio_capture', { request: { purpose, intentId } }),
    stop: (sessionId: string, intentId: string) =>
      boundary.invoke<AudioArtifact>('stop_audio_capture', { request: { sessionId, intentId } }),
    cancel: (intentId: string, sessionId: string | undefined, reason: string) =>
      boundary.invoke<void>('cancel_audio_capture', { request: { intentId, sessionId, reason } }),
    listen: (listener: (event: CaptureEvent) => void) =>
      boundary.listen<CaptureEvent>('echo-audio-capture', (event) => listener(event.payload))
  };
}

export type NativeCaptureClient = ReturnType<typeof createNativeCaptureClient>;

export function isNativeCaptureError(error: unknown): error is NativeCaptureError {
  if (!error || typeof error !== 'object') return false;
  return 'code' in error && typeof error.code === 'string' &&
    'phase' in error && typeof error.phase === 'string' &&
    'retryable' in error && typeof error.retryable === 'boolean' &&
    'stage' in error && typeof error.stage === 'string';
}
