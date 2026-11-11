import type { TranscriptRecord } from '#lib/core/transcript.ts';
import type { ArtifactProof } from '#lib/runtime/desktop/native-transcription-client.ts';

export type TranscriptDeliveryTask = {
  delivery: { pasteAfterTranscription: boolean; restoreClipboard: boolean };
  persistence: { errorCode?: 'history_write_failed'; saved: boolean };
  proof: ArtifactProof;
  transcript: TranscriptRecord;
};

type TranscriptDeliveryQueueOptions = {
  deliver(task: TranscriptDeliveryTask): Promise<void>;
};

/** Orders clipboard/paste delivery while allowing native transcription jobs to overlap. */
export function createTranscriptDeliveryQueue(options: TranscriptDeliveryQueueOptions) {
  let deliveryTail = Promise.resolve();
  let isDisposed = false;

  return {
    enqueue(transcription: Promise<TranscriptDeliveryTask>) {
      const settled = transcription.then(
        (task) => ({ ok: true as const, task }),
        (error: unknown) => ({ error, ok: false as const })
      );
      const delivery = deliveryTail.then(async () => {
        const result = await settled;
        if (!result.ok) throw result.error;
        if (!isDisposed) await options.deliver(result.task);
      });
      deliveryTail = delivery.catch(() => undefined);
      return delivery;
    },
    dispose() {
      isDisposed = true;
    }
  };
}
