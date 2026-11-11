import {
  isNativeCaptureError,
  type AudioArtifact,
  type CapturePurpose,
  type CaptureEvent,
  type CaptureSession,
  type NativeCaptureClient
} from '#lib/runtime/desktop/native-capture-client.ts';

type CaptureAttempt = {
  automaticSessionId?: string;
  finalization?: Promise<CaptureStopOutcome>;
  intentId: string;
  isHeld: boolean;
  session?: CaptureSession;
  start: Promise<CaptureStartOutcome>;
};

type CaptureStartOutcome =
  | { status: 'recording'; session: CaptureSession }
  | { status: 'cancelled'; intentId?: string }
  | { status: 'duplicate'; intentId?: string };

type CaptureStopOutcome =
  | { status: 'captured'; artifact: AudioArtifact; intentId: string }
  | { status: 'cancelled'; intentId?: string }
  | { status: 'duplicate'; intentId?: string };

type CaptureIntentControllerOptions = {
  client: NativeCaptureClient;
  createIntentId?: () => string;
  onIntent?(intentId: string): void;
  purpose: CapturePurpose;
};

/** Owns renderer press/release intent while Rust owns capture truth and audio. */
export function createCaptureIntentController(options: CaptureIntentControllerOptions) {
  return new CaptureIntentController(options);
}

class CaptureIntentController {
  private attempt: CaptureAttempt | undefined;
  private captureBarrier = Promise.resolve();
  private isDisposed = false;
  private readonly createIntentId: () => string;

  constructor(private readonly options: CaptureIntentControllerOptions) {
    this.createIntentId = options.createIntentId ?? (() => crypto.randomUUID());
  }

  press(): Promise<CaptureStartOutcome> {
    if (this.isDisposed) {
      return Promise.resolve({ status: 'cancelled', intentId: this.attempt?.intentId });
    }
    if (this.attempt?.isHeld) {
      return Promise.resolve({ status: 'duplicate', intentId: this.attempt.intentId });
    }
    const candidate = {
      intentId: this.createIntentId(),
      isHeld: true,
      start: undefined as unknown as Promise<CaptureStartOutcome>
    };
    this.attempt = candidate;
    this.options.onIntent?.(candidate.intentId);
    candidate.start = this.captureBarrier.then(() => this.begin(candidate));
    this.setBarrier(candidate.start);
    return candidate.start;
  }

  matches(event: CaptureEvent) {
    // Native start failures arrive before start() rejects and must keep that rejection visible.
    if (event.phase === 'idle' && event.error
      && (event.outcome === 'start_failed' || event.outcome === 'first_callback_timeout')) return false;
    return !!this.attempt?.isHeld && event.intentId === this.attempt.intentId
      && event.sessionId !== null
      && (!this.attempt.session || event.sessionId === this.attempt.session.sessionId);
  }

  release(event?: CaptureEvent): Promise<CaptureStopOutcome> {
    const candidate = this.attempt;
    if (!candidate?.isHeld || (event && !this.matches(event))) {
      return Promise.resolve({ status: 'duplicate', intentId: candidate?.intentId });
    }
    candidate.isHeld = false;
    candidate.automaticSessionId = event?.sessionId ?? undefined;
    candidate.finalization ??= this.finalize(candidate, event?.sessionId ?? undefined);
    this.setBarrier(candidate.finalization);
    return candidate.finalization;
  }

  async cancel(reason = 'renderer_cancelled') {
    const candidate = this.attempt;
    if (!candidate) return;
    candidate.isHeld = false;
    this.attempt = undefined;
    await this.options.client.cancel(candidate.intentId, candidate.session?.sessionId, reason);
    await candidate.start.catch(() => undefined);
  }

  isHeld() {
    return this.attempt?.isHeld ?? false;
  }

  async dispose() {
    if (this.isDisposed) return;
    this.isDisposed = true;
    const candidate = this.attempt;
    this.attempt = undefined;
    if (!candidate) return;
    candidate.isHeld = false;
    const cancellation = this.options.client
      .cancel(candidate.intentId, candidate.session?.sessionId, 'renderer_disposed')
      .catch(() => undefined);
    await Promise.all([cancellation, candidate.start.catch(() => undefined)]);
  }

  private async begin(candidate: CaptureAttempt): Promise<CaptureStartOutcome> {
    if (!this.isCurrent(candidate) || !candidate.isHeld) {
      return { status: 'cancelled', intentId: candidate.intentId };
    }
    try {
      const session = await this.options.client.start(this.options.purpose, candidate.intentId);
      if (!this.isCurrent(candidate) || (!candidate.isHeld && candidate.automaticSessionId !== session.sessionId)) {
        await this.options.client.cancel(candidate.intentId, session.sessionId, 'released_during_start');
        return { status: 'cancelled', intentId: candidate.intentId };
      }
      candidate.session = session;
      return { status: 'recording', session };
    } catch (error) {
      if (!this.isCurrent(candidate) || !candidate.isHeld || isCancellation(error)) {
        return { status: 'cancelled', intentId: candidate.intentId };
      }
      throw error;
    }
  }

  private async finalize(candidate: CaptureAttempt, automaticSessionId?: string): Promise<CaptureStopOutcome> {
    const sessionId = candidate.session?.sessionId ?? automaticSessionId;
    if (!sessionId) {
      await this.options.client.cancel(candidate.intentId, undefined, 'released_before_start');
      await candidate.start;
      return { status: 'cancelled', intentId: candidate.intentId };
    }
    const artifact = await this.options.client.stop(sessionId, candidate.intentId);
    return { status: 'captured', artifact, intentId: candidate.intentId };
  }

  private isCurrent(candidate: CaptureAttempt) {
    return !this.isDisposed && this.attempt === candidate;
  }

  private setBarrier(result: Promise<unknown>) {
    this.captureBarrier = result.then(() => undefined, () => undefined);
  }
}

function isCancellation(error: unknown) {
  return isNativeCaptureError(error) && error.code === 'capture_cancelled';
}
