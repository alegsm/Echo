import type { EchoErrorCode } from '#lib/core/types.ts';
import type { NativeTranscriptionClient } from '#lib/runtime/desktop/native-transcription-client.ts';
import type { createDesktopTranscriptDelivery } from '#lib/runtime/desktop/transcript-delivery.ts';
import {
  createTranscriptDeliveryQueue,
  type TranscriptDeliveryTask
} from '#lib/runtime/desktop/transcript-delivery-queue.ts';

type ReportedDeliveryOptions = {
  client: NativeTranscriptionClient;
  delivery: ReturnType<typeof createDesktopTranscriptDelivery>;
};

/** Reports the one-shot native history outcome after ordered desktop delivery. */
export function createReportedTranscriptDelivery(options: ReportedDeliveryOptions) {
  return createTranscriptDeliveryQueue({ deliver: (task) => deliverAndReport(options, task) });
}

async function deliverAndReport(options: ReportedDeliveryOptions, task: TranscriptDeliveryTask) {
  if (task.transcript.errorCode === 'provider_failed' && task.transcript.insertionMode === 'failed') return;
  let insertionMode: 'pasted' | 'copied';
  try {
    insertionMode = await options.delivery.deliver(task.transcript, task.delivery);
  } catch (error) {
    if (task.persistence.saved) await reportFailure(options.client, task, error);
    throw error;
  }

  if (!task.persistence.saved) return;
  await options.client.reportDelivery({
    insertionMode,
    intentId: task.proof.intentId,
    sessionId: task.proof.sessionId,
    transcriptId: task.transcript.id
  });
}

function reportFailure(
  client: NativeTranscriptionClient,
  task: TranscriptDeliveryTask,
  error: unknown
) {
  return client.reportDelivery({
    errorCode: deliveryErrorCode(error),
    insertionMode: 'failed',
    intentId: task.proof.intentId,
    sessionId: task.proof.sessionId,
    transcriptId: task.transcript.id
  }).catch(() => undefined);
}

function deliveryErrorCode(error: unknown): 'clipboard_failed' | 'insertion_failed' {
  return errorCode(error) === 'clipboard_failed' ? 'clipboard_failed' : 'insertion_failed';
}

function errorCode(error: unknown): EchoErrorCode | undefined {
  if (!error || typeof error !== 'object' || !('code' in error)) return undefined;
  return String(error.code) as EchoErrorCode;
}
