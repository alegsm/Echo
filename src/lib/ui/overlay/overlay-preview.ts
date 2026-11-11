import type { EchoRuntimeEvent } from '#lib/core/types.ts';

export const PREVIEW_FADE_MS = 200;
const PREVIEW_CHARACTER_MS = 12;
const PREVIEW_CHUNK_MAX_MS = 900;
export const PREVIEW_CARET_FADE_MS = 300;

type PreviewWordRow = { bottom: number; revealAt: number };

/** Centres short revealed content; taller content pins its newest row to the clip bottom. */
export function previewContentOffset(rows: PreviewWordRow[], clipHeight: number, now: number, reducedMotion = false): number {
  let bottom = clipHeight;
  for (const row of rows) {
    if (reducedMotion || row.revealAt <= now) bottom = row.bottom;
  }
  const remaining = clipHeight - bottom;
  return remaining >= 0 ? remaining / 2 : remaining;
}

export type PreviewWord = {
  id: string;
  text: string;
  chunkOrdinal: number;
  arrivedAt: number;
  revealDelayMs: number;
  revealDurationMs: number;
};

/** A remounted word resumes its original timeline, or stays fully revealed. */
export function previewWordDelay(word: PreviewWord, now: number, reducedMotion = false): number | null {
  const delay = word.arrivedAt + word.revealDelayMs - now;
  return reducedMotion || delay <= -word.revealDurationMs ? null : delay;
}

type PreviewWordAnimation = { wordId: string; delayMs: number | null };

/** A mounted CSS animation keeps its initial delay because its start time is fixed. */
export function previewWordAnimation(
  word: PreviewWord, now: number, reducedMotion = false, scheduled?: PreviewWordAnimation
): PreviewWordAnimation {
  if (scheduled?.wordId === word.id && (!reducedMotion || scheduled.delayMs === null)) return scheduled;
  return { wordId: word.id, delayMs: previewWordDelay(word, now, reducedMotion) };
}

export type PreviewCaret = { wordIndex: number; progress: number; opacity: number };

/** The writing edge holds across separator intervals, then fades without blinking. */
export function previewCaretState(words: PreviewWord[], now: number, reducedMotion = false, out?: PreviewCaret): PreviewCaret | null {
  if (reducedMotion) return null;
  for (let wordIndex = words.length - 1; wordIndex >= 0; wordIndex--) {
    const word = words[wordIndex];
    const start = word.arrivedAt + word.revealDelayMs;
    if (now < start) continue;
    const progress = Math.min((now - start) / word.revealDurationMs, 1);
    const end = start + word.revealDurationMs;
    const opacity = wordIndex === words.length - 1
      ? Math.max(0, 1 - Math.max(0, now - end) / PREVIEW_CARET_FADE_MS) : 1;
    if (opacity <= 0) return null;
    const result = out ?? { wordIndex: 0, progress: 0, opacity: 0 };
    result.wordIndex = wordIndex;
    result.progress = progress;
    result.opacity = opacity;
    return result;
  }
  return null;
}

type OverlayPreview = {
  dictationId?: string;
  generation: number;
  text: string;
  words: PreviewWord[];
  ordinal: number;
  phase: 'hidden' | 'visible' | 'fading';
  cleared: boolean;
};

export const emptyPreview: OverlayPreview = {
  generation: 0, text: '', words: [], ordinal: -1, phase: 'hidden', cleared: false
};

type PreviewEvent = EchoRuntimeEvent | {
  type: 'fade_finished';
  dictationId: string;
  generation: number;
};

/** Release clears display text immediately; delivery during capture keeps its guarded fade. */
export function reduceOverlayPreview(state: OverlayPreview, event: PreviewEvent, now = performance.now()): OverlayPreview {
  if (event.type === 'state_changed' && event.snapshot.state !== 'recording' &&
      event.snapshot.state !== 'requesting_microphone') {
    if (!state.dictationId || state.cleared) return state;
    return { ...state, text: '', words: [], phase: 'hidden', cleared: true, generation: state.generation + 1 };
  }
  if (event.type === 'partial_transcript_changed') {
    const projection = event.projection;
    if (projection.phase === 'active') {
      const isNew = projection.dictationId !== state.dictationId;
      // Only a capture-begin projection can replace an existing dictation.
      if (isNew && state.dictationId && projection.ordinal !== -1) return state;
      if (!isNew && (state.cleared || state.phase === 'fading')) return state;
      if (!isNew && projection.ordinal < state.ordinal) return state;
      const text = projection.eligible ? projection.text.trim() : '';
      if (!isNew && text === state.text && projection.ordinal === state.ordinal) return state;
      const previous = isNew ? [] : state.words;
      const suffix = text.slice(isNew ? 0 : state.text.length).trim();
      const appended = suffix ? suffix.split(/\s+/u) : [];
      const characterCount = appended.reduce((count, word) => count + Array.from(word).length, 0)
        + Math.max(0, appended.length - 1);
      const intervalMs = characterCount ? Math.min(PREVIEW_CHARACTER_MS, PREVIEW_CHUNK_MAX_MS / characterCount) : 0;
      const last = previous.at(-1);
      const previousEnd = last ? last.arrivedAt + last.revealDelayMs + last.revealDurationMs : now;
      let revealDelayMs = Math.max(0, previousEnd - now);
      const words = [...previous, ...appended.map((word, index) => {
        const revealDurationMs = Array.from(word).length * intervalMs;
        const scheduled = {
          id: JSON.stringify([projection.dictationId, projection.ordinal, index]),
          text: word,
          chunkOrdinal: projection.ordinal,
          arrivedAt: now,
          revealDelayMs,
          revealDurationMs
        };
        revealDelayMs += revealDurationMs + intervalMs;
        return scheduled;
      })];
      return {
        dictationId: projection.dictationId,
        generation: state.generation + (isNew ? 1 : 0),
        text,
        words,
        ordinal: projection.ordinal,
        phase: text ? 'visible' : 'hidden',
        cleared: false
      };
    }
    if (projection.dictationId !== state.dictationId || projection.phase === 'completed' || state.cleared) return state;
    return { ...state, text: '', words: [], phase: 'hidden', cleared: true, generation: state.generation + 1 };
  }
  if (event.type === 'transcript_completed') {
    if (!event.dictationId || event.dictationId !== state.dictationId || !state.text || state.phase === 'fading') return state;
    return { ...state, phase: 'fading', generation: state.generation + 1 };
  }
  if (event.type === 'fade_finished' && event.dictationId === state.dictationId &&
      event.generation === state.generation && state.phase === 'fading') {
    return { ...state, text: '', words: [], phase: 'hidden', cleared: true };
  }
  return state;
}
