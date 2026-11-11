import type { EchoErrorCode } from './types';

type InsertionMode = 'pasted' | 'copied' | 'manual' | 'failed';

type TranscriptTiming = {
  recordingMs: number;
  blobReadyMs: number;
  uploadMs: number;
  stopToTranscriptMs: number;
  historyWriteMs?: number;
  providerMs?: number;
  backendMs?: number;
};

type TranscriptAudioMetadata = {
  mimeType: string;
  byteSize: number;
};

export type TranscriptRecord = {
  id: string;
  text: string;
  createdAt: string;
  durationMs: number;
  providerId: string;
  providerModel?: string;
  language?: string;
  prompt?: string;
  timings: TranscriptTiming;
  audio: TranscriptAudioMetadata;
  insertionMode: InsertionMode;
  /** History retains browser records created before the desktop-only runtime. */
  source: 'web' | 'desktop';
  errorCode?: EchoErrorCode;
};
