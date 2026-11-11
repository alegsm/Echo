import type { CaptureEvent, NativeCaptureBoundary } from '#lib/runtime/desktop/native-capture-client.ts';
import type { PartialTranscriptProjection, PartialTranscriptStatus } from '#lib/core/types.ts';

type PartialTranscript = { dictationId: string; ordinal: number; text: string; status?: PartialTranscriptStatus };

/** Retains overlay preview text; final transcript delivery uses a separate path. */
export function createPartialTranscriptState(onChanged: (projection: PartialTranscriptProjection) => void = () => undefined) {
  let current: PartialTranscript | undefined;
  let projection: PartialTranscriptProjection | undefined;
  let isDisposed = false;
  let captureSequence = -1;
  let nativeCaptureId: string | undefined;
  const unlisteners: (() => void)[] = [];

  function clear(dictationId: string | undefined = projection?.dictationId, phase: 'completed' | 'cleared' = 'cleared') {
    if (isDisposed || !projection || projection.dictationId !== dictationId || projection.phase === 'cleared') return;
    current = undefined;
    publish({ ...projection, text: '', phase });
  }

  function publish(next: PartialTranscriptProjection) {
    projection = next;
    onChanged(next);
  }

  function apply(event: PartialTranscript) {
    if (isDisposed || !current || event.dictationId !== current.dictationId) return;
    if (event.status) {
      publish({ ...projection!, status: event.status, statusOrdinal: event.ordinal, eligible: true });
      return;
    }
    if (event.ordinal <= current.ordinal) return;
    current = {
      dictationId: event.dictationId,
      ordinal: event.ordinal,
      text: [current.text, event.text.trim()].filter(Boolean).join(' ')
    };
    publish({ ...current, eligible: projection!.eligible, phase: 'active' });
  }

  async function listen<T>(boundary: NativeCaptureBoundary, name: string, listener: (event: T) => void) {
    const unlisten = await boundary.listen<T>(name, (event) => listener(event.payload));
    if (isDisposed) unlisten();
    else unlisteners.push(unlisten);
  }

  return {
    async start(boundary: NativeCaptureBoundary, onCapture?: (event: CaptureEvent) => void) {
      await listen<PartialTranscript>(boundary, 'echo-partial-transcript', apply);
      if (isDisposed) return;
      await listen<CaptureEvent>(boundary, 'echo-audio-capture', (event) => {
        if (event.sequence <= captureSequence) return;
        captureSequence = event.sequence;
        onCapture?.(event);
        nativeCaptureId = event.intentId ?? nativeCaptureId;
        if (event.phase === 'idle' && event.error && nativeCaptureId) clear(nativeCaptureId);
        if (event.outcome === 'segment_closed' && event.intentId === current?.dictationId && projection && !projection.eligible) {
          publish({ ...projection, eligible: true });
        }
        if (event.phase === 'idle') nativeCaptureId = undefined;
      });
      if (isDisposed) return;
      await listen<{ intentId: string; state: string }>(boundary, 'echo-stt-job', (event) => {
        if (['failed', 'canceled', 'completed'].includes(event.state)) {
          clear(event.intentId, event.state === 'completed' ? 'completed' : 'cleared');
        }
      });
    },
    begin(dictationId: string) {
      if (isDisposed) return;
      current = { dictationId, ordinal: -1, text: '' };
      publish({ ...current, eligible: false, phase: 'active' });
    },
    apply,
    clear,
    get: () => current,
    getProjection: () => projection,
    dispose() {
      isDisposed = true;
      current = undefined;
      projection = undefined;
      for (const unlisten of unlisteners.splice(0)) unlisten();
    }
  };
}
