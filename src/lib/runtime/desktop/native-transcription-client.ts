import type { TranscriptRecord } from '#lib/core/transcript.ts';
import type { EchoSnapshot } from '#lib/core/types.ts';
import type { NativeCaptureBoundary } from '#lib/runtime/desktop/native-capture-client.ts';

export type ArtifactProof = {
  artifactId: string;
  intentId: string;
  sessionId: string;
};

type SttJobErrorCode =
  | 'invalid_job' | 'job_canceled' | 'too_many_jobs' | 'unsupported_provider'
  | 'missing_provider_key' | 'credential_unavailable' | 'provider_credential_rejected'
  | 'artifact_not_found' | 'artifact_expired' | 'artifact_busy' | 'invalid_audio'
  | 'provider_timeout' | 'provider_failed' | 'provider_invalid_response'
  | 'provider_empty_transcript' | 'provider_low_confidence' | 'history_write_failed';

type SttJobError = {
  code: SttJobErrorCode;
  message: string;
  retryable: boolean;
  stage: 'settings' | 'credential' | 'artifact' | 'provider' | 'history';
};

export type NativeTranscriptionResult = {
  delivery: { pasteAfterTranscription: boolean; restoreClipboard: boolean };
  jobSequence: number;
  persistence: { errorCode?: 'history_write_failed'; saved: boolean };
  snapshot: { sequence: number; snapshot: EchoSnapshot };
  transcript: TranscriptRecord;
};

type TranscriptDeliveryReport = {
  errorCode?: 'clipboard_failed' | 'insertion_failed';
  intentId: string;
  insertionMode: 'pasted' | 'copied' | 'failed';
  sessionId: string;
  transcriptId: string;
};

/** Starts native-owned transcription and history persistence using artifact identity only. */
export function createNativeTranscriptionClient(boundary: Pick<NativeCaptureBoundary, 'invoke'>) {
  return {
    transcribe(proof: ArtifactProof) {
      return boundary.invoke<NativeTranscriptionResult>('transcribe_audio_artifact', { request: proof });
    },
    cancel(sessionId: string, intentId: string) {
      return boundary.invoke<{
        canceled: boolean;
        snapshot: { sequence: number; snapshot: EchoSnapshot };
      }>(
        'cancel_transcription_job',
        { request: { sessionId, intentId } }
      );
    },
    reportDelivery(report: TranscriptDeliveryReport) {
      return boundary.invoke<{
        historyRevision: number;
        transcript: { record: TranscriptRecord; revision: number };
      }>('report_transcript_delivery', { request: report });
    }
  };
}

export type NativeTranscriptionClient = ReturnType<typeof createNativeTranscriptionClient>;

export function isSttJobError(error: unknown): error is SttJobError {
  if (!error || typeof error !== 'object') return false;
  return 'code' in error && typeof error.code === 'string' &&
    'message' in error && typeof error.message === 'string' &&
    'retryable' in error && typeof error.retryable === 'boolean' &&
    'stage' in error && typeof error.stage === 'string';
}
