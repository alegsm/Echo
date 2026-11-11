import type { TranscriptRecord } from '#lib/core/transcript.ts';
import type { EchoErrorCode } from '#lib/core/types.ts';
import { recordDiagnosticEvent } from '#lib/runtime/debug-logging.ts';

type TranscriptDeliveryOptions = {
  boundary: { invoke<T>(command: string, args: Record<string, unknown>): Promise<T> };
};

/** Native commands own clipboard contents and paste sequencing in both desktop windows. */
export function createDesktopTranscriptDelivery(options: TranscriptDeliveryOptions) {
  return {
    async copy(transcript: TranscriptRecord) {
      try {
        await options.boundary.invoke<void>('copy_transcript', { request: { text: transcript.text } });
        void diagnostic('clipboard_copy_completed', 'clipboard', 'success');
      } catch {
        void diagnostic('clipboard_copy_failed', 'clipboard', 'failed', 'clipboard_failed');
        throw deliveryError('clipboard_failed');
      }
    },
    async deliver(
      transcript: TranscriptRecord,
      policy: { pasteAfterTranscription: boolean; restoreClipboard: boolean }
    ) {
      try {
        const outcome = await options.boundary.invoke<'pasted' | 'copied'>('deliver_transcript', {
          request: { text: transcript.text, ...policy }
        });
        void diagnostic('clipboard_copy_completed', 'clipboard', 'success');
        if (outcome === 'copied') void diagnostic('paste_suppressed', 'paste', 'disabled');
        else void diagnostic('paste_completed', 'paste', 'success');
        return outcome;
      } catch (error) {
        const code = isInsertionFailure(error) ? 'insertion_failed' : 'clipboard_failed';
        if (code === 'insertion_failed') {
          void diagnostic('clipboard_copy_completed', 'clipboard', 'success');
          void diagnostic('paste_failed', 'paste', 'failed', code);
        } else void diagnostic('clipboard_copy_failed', 'clipboard', 'failed', code);
        throw deliveryError(code);
      }
    }
  };
}

function diagnostic(event: string, stage: string, outcome: string, code?: string) {
  return recordDiagnosticEvent({ area: 'recording', event, fields: { code, outcome, stage } });
}

function isInsertionFailure(error: unknown) {
  return error !== null && typeof error === 'object' && 'code' in error && error.code === 'insertion_failed';
}

function deliveryError(code: Extract<EchoErrorCode, 'clipboard_failed' | 'insertion_failed'>) {
  return Object.assign(new Error(code === 'clipboard_failed'
    ? 'Transcript could not be copied.' : 'Transcript could not be pasted.'), { code });
}
