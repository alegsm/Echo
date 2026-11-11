import type { EchoSnapshot } from '#lib/core/types.ts';
import type { TranscriptRecord } from '#lib/core/transcript.ts';
import type { createCaptureIntentController } from '#lib/runtime/desktop/capture-intent-controller.ts';
import type { AudioArtifact, CaptureEvent } from '#lib/runtime/desktop/native-capture-client.ts';
import type { createNativeRuntimeState } from '#lib/runtime/desktop/native-runtime-state.ts';
import type {
  ArtifactProof,
  NativeTranscriptionClient,
  NativeTranscriptionResult
} from '#lib/runtime/desktop/native-transcription-client.ts';
import type { createTranscriptDeliveryQueue } from '#lib/runtime/desktop/transcript-delivery-queue.ts';

type DictationControllerOptions = {
  capture: ReturnType<typeof createCaptureIntentController>;
  delivery: ReturnType<typeof createTranscriptDeliveryQueue>;
  onPersistenceError(): void;
  onTranscript(transcript: TranscriptRecord, dictationId: string): void;
  state: ReturnType<typeof createNativeRuntimeState>;
  transcription: NativeTranscriptionClient;
};

/** Coordinates metadata-only PTT capture, native STT jobs and disposal. */
export function createDictationController(options: DictationControllerOptions) {
  return new DictationController(options);
}

class DictationController {
  private activeCaptureOrdinal: number | undefined;
  private isDisposed = false;
  private readonly jobs = new Map<string, { sessionId: string }>();
  private latestCaptureOrdinal = 0;
  constructor(private readonly options: DictationControllerOptions) {}

  async start() {
    if (this.options.capture.isHeld()) {
      await this.options.capture.press();
      return this.refreshSnapshot();
    }
    const ordinal = ++this.latestCaptureOrdinal;
    this.activeCaptureOrdinal = ordinal;
    const outcome = await this.options.capture.press();
    if (outcome.status !== 'recording' && this.activeCaptureOrdinal === ordinal) {
      this.activeCaptureOrdinal = undefined;
    }
    return this.refreshSnapshot();
  }

  matchesCapture(event: CaptureEvent) {
    return this.options.capture.matches(event);
  }

  async stop(event?: CaptureEvent) {
    const activeOrdinal = this.activeCaptureOrdinal;
    const outcome = await this.options.capture.release(event);
    if (outcome.status !== 'captured') {
      if (activeOrdinal !== undefined && this.activeCaptureOrdinal === activeOrdinal) {
        this.activeCaptureOrdinal = undefined;
      }
      return this.refreshSnapshot();
    }
    const ordinal = activeOrdinal ?? ++this.latestCaptureOrdinal;
    if (this.activeCaptureOrdinal === ordinal) this.activeCaptureOrdinal = undefined;
    const proof = artifactProof(outcome.artifact, outcome.intentId);
    return this.runTranscription(proof, ordinal);
  }

  async cancel() {
    const ordinal = this.activeCaptureOrdinal;
    await this.options.capture.cancel('dictation_cancelled');
    if (this.activeCaptureOrdinal === ordinal) this.activeCaptureOrdinal = undefined;
    return this.refreshSnapshot();
  }

  async dispose() {
    if (this.isDisposed) return;
    this.isDisposed = true;
    this.options.delivery.dispose();
    await Promise.all([this.options.capture.dispose(), this.cancelJobs()]);
  }

  private async runTranscription(
    proof: ArtifactProof,
    ordinal: number
  ): Promise<EchoSnapshot | undefined> {
    const request = this.options.transcription.transcribe(proof);
    this.jobs.set(proof.intentId, { sessionId: proof.sessionId });
    const deliveryTask = request.then((value) => ({
      delivery: value.delivery,
      persistence: value.persistence,
      proof,
      transcript: value.transcript
    }));
    const delivered = this.options.delivery.enqueue(deliveryTask);
    const deliveryOutcome = delivered.then(
      () => ({ ok: true as const }),
      (error: unknown) => ({ error, ok: false as const })
    );
    try {
      const completed = await request;
      this.applyCompletion(completed, proof.intentId);
      if (!completed.persistence.saved && ordinal >= this.latestCaptureOrdinal) {
        this.options.onPersistenceError();
      }
      const deliveryResult = await deliveryOutcome;
      if (!deliveryResult.ok) throw deliveryResult.error;
      return completed.snapshot.snapshot;
    } catch (error) {
      const snapshot = await this.refreshSnapshot();
      if (ordinal < this.latestCaptureOrdinal) return snapshot;
      throw error;
    } finally {
      this.jobs.delete(proof.intentId);
    }
  }

  private applyCompletion(result: NativeTranscriptionResult, dictationId: string) {
    if (this.isDisposed) return;
    this.options.state.apply(result.snapshot);
    this.options.onTranscript(result.transcript, dictationId);
  }

  private async refreshSnapshot() {
    await this.options.state.refresh();
    return this.options.state.getSnapshot();
  }

  private async cancelJobs() {
    const jobs = [...this.jobs.entries()];
    await Promise.all(jobs.map(([intentId, job]) => {
      return this.options.transcription.cancel(job.sessionId, intentId).catch(() => undefined);
    }));
  }
}

function artifactProof(
  artifact: AudioArtifact,
  intentId: string
): ArtifactProof {
  return {
    artifactId: artifact.artifactId,
    intentId,
    sessionId: artifact.sessionId
  };
}
