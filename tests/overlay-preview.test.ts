import { expect, test } from 'bun:test';
import type { EchoRuntimeEvent, PartialTranscriptProjection } from '../src/lib/core/types';
import { emptyPreview, previewContentOffset, reduceOverlayPreview } from '../src/lib/ui/overlay/overlay-preview';
import * as previewTiming from '../src/lib/ui/overlay/overlay-preview';

function partial(overrides: Partial<PartialTranscriptProjection> = {}): EchoRuntimeEvent {
  return {
    type: 'partial_transcript_changed',
    projection: { dictationId: 'a', ordinal: 0, text: 'words', eligible: true, phase: 'active', ...overrides }
  };
}

function completed(dictationId?: string): EchoRuntimeEvent {
  return { type: 'transcript_completed', dictationId, transcript: {} as never };
}

test.each(['', '   '])('empty eligible text %j keeps the pill hidden', (text) => {
  expect(reduceOverlayPreview(emptyPreview, partial({ text })).text).toBe('');
});

test('release-tail text stays hidden until a segment closure makes the projection eligible', () => {
  const hidden = reduceOverlayPreview(emptyPreview, partial({ eligible: false }));
  expect(hidden.text).toBe('');
  const visible = reduceOverlayPreview(hidden, partial());
  expect(visible).toMatchObject({ dictationId: 'a', text: 'words', phase: 'visible' });
  expect(reduceOverlayPreview(visible, partial({ ordinal: 2, text: 'words arrive' })).text).toBe('words arrive');
});

test('native completion retains text until matching delivery starts a generation-guarded fade', () => {
  const visible = reduceOverlayPreview(emptyPreview, partial());
  const retained = reduceOverlayPreview(visible, partial({ text: '', phase: 'completed' }));
  expect(retained).toEqual(visible);
  expect(reduceOverlayPreview(retained, completed())).toEqual(visible);
  expect(reduceOverlayPreview(retained, completed('old'))).toEqual(visible);
  const fading = reduceOverlayPreview(retained, completed('a'));
  expect(fading).toMatchObject({ text: 'words', phase: 'fading' });
  const finish = { type: 'fade_finished' as const, dictationId: 'a', generation: fading.generation };
  expect(reduceOverlayPreview(fading, { ...finish, generation: fading.generation - 1 })).toEqual(fading);
  expect(reduceOverlayPreview(fading, finish).text).toBe('');
  const newer = reduceOverlayPreview(fading, partial({ dictationId: 'b', ordinal: -1, text: '', eligible: false }));
  expect(reduceOverlayPreview(newer, finish)).toEqual(newer);
  const newerVisible = reduceOverlayPreview(newer, partial({ dictationId: 'b', text: 'new words' }));
  expect(reduceOverlayPreview(newerVisible, completed('a'))).toEqual(newerVisible);
  expect(reduceOverlayPreview(newerVisible, partial({ ordinal: 2, text: 'late old words' }))).toEqual(newerVisible);
  expect(reduceOverlayPreview(newerVisible, partial({ phase: 'cleared', text: '' }))).toEqual(newerVisible);
});

test('chunks append stable words with character durations and separator intervals', () => {
  const first = reduceOverlayPreview(emptyPreview, partial({ text: 'first words' }), 1000);
  expect(first.words.map(({ text, chunkOrdinal, arrivedAt, revealDelayMs, revealDurationMs }) =>
    [text, chunkOrdinal, arrivedAt, revealDelayMs, revealDurationMs])).toEqual([
    ['first', 0, 1000, 0, 60], ['words', 0, 1000, 72, 60]
  ]);
  const second = reduceOverlayPreview(first, partial({ ordinal: 2, text: 'first words then more' }), 2000);
  expect(second.words.slice(0, 2)).toEqual(first.words);
  expect(second.words[0]).toBe(first.words[0]);
  expect(second.words.slice(2).map(({ chunkOrdinal, revealDelayMs }) => [chunkOrdinal, revealDelayMs]))
    .toEqual([[2, 0], [2, 60]]);
  expect(new Set(second.words.map(({ id }) => id)).size).toBe(4);
  expect(reduceOverlayPreview(second, partial({ ordinal: 0, text: 'first words' }), 3000)).toBe(second);
  expect(reduceOverlayPreview(second, partial({ ordinal: 2, text: second.text }), 3000)).toBe(second);
});

test.each(['a', '😀 a', 'x'.repeat(75), 'x'.repeat(76), Array(50).fill('word').join(' ')])(
  'chunk budget counts code points and spaces: %s', (text) => {
    const { words } = reduceOverlayPreview(emptyPreview, partial({ text }), 1000);
    const characters = Array.from(text).length;
    const interval = Math.min(12, 900 / characters);
    let cursor = 0;
    for (const word of words) {
      expect(word.revealDelayMs).toBeCloseTo(cursor);
      expect(word.revealDurationMs).toBeCloseTo(Array.from(word.text).length * interval);
      cursor += word.revealDurationMs + interval;
    }
    const last = words.at(-1)!;
    expect(last.revealDelayMs + last.revealDurationMs).toBeCloseTo(Math.min(12 * characters, 900));
  }
);

test('a chunk arriving during typing queues after the previous word ends', () => {
  const first = reduceOverlayPreview(emptyPreview, partial({ text: 'first words' }), 1000);
  const queued = reduceOverlayPreview(first, partial({ ordinal: 1, text: 'first words next' }), 1050);
  expect(queued.words[2]).toMatchObject({ arrivedAt: 1050, revealDelayMs: 82, revealDurationMs: 48 });
  expect(queued.words[0]).toBe(first.words[0]);
});

test('remounted words retain future, partial and completed wipe timing', () => {
  const { words } = reduceOverlayPreview(emptyPreview, partial({ text: 'first words' }), 1000);
  expect(previewTiming.previewWordDelay(words[1], 1050)).toBe(22);
  expect(previewTiming.previewWordDelay(words[0], 1030)).toBe(-30);
  expect(previewTiming.previewWordDelay(words[0], 1060)).toBeNull();
  expect(previewTiming.previewWordDelay(words[1], 1000, true)).toBeNull();
});

test('model updates preserve a mounted word animation delay while a fresh mount resumes the clock', () => {
  const { words } = reduceOverlayPreview(emptyPreview, partial({ text: 'first words' }), 1000);
  const initial = previewTiming.previewWordAnimation(words[1], 1050);
  expect(initial.delayMs).toBe(22);
  expect(previewTiming.previewWordAnimation(words[1], 1100, false, initial).delayMs).toBe(22);
  expect(previewTiming.previewWordAnimation(words[1], 1200, false, initial).delayMs).toBe(22);
  expect(previewTiming.previewWordAnimation(words[1], 1100).delayMs).toBe(-28);
  expect(previewTiming.previewWordAnimation(words[0], 1050, false, initial).delayMs).toBe(-50);
  expect(previewTiming.previewWordAnimation(words[1], 1050, true).delayMs).toBeNull();
});

test('caret interpolates the active word, holds across spaces, then fades for 300 ms', () => {
  const { words } = reduceOverlayPreview(emptyPreview, partial({ text: 'first words' }), 1000);
  const caret = previewTiming.previewCaretState;
  expect(caret(words, 999)).toBeNull();
  for (const [now, wordIndex, progress, opacity] of [
    [1000, 0, 0, 1], [1015, 0, 0.25, 1], [1030, 0, 0.5, 1],
    [1045, 0, 0.75, 1], [1066, 0, 1, 1], [1072, 1, 0, 1],
    [1132, 1, 1, 1], [1282, 1, 1, 0.5]
  ]) {
    expect(caret(words, now)).toEqual({ wordIndex, progress, opacity });
  }
  expect(caret(words, 1432)).toBeNull();
  expect(caret(words, 1500)).toBeNull();
  expect(caret(words, 1030, true)).toBeNull();
  expect(caret([], 1030)).toBeNull();
});

test('a new chunk restores the caret during its fade and clearing suppresses it', () => {
  const first = reduceOverlayPreview(emptyPreview, partial({ text: 'first' }), 1000);
  expect(previewTiming.previewCaretState(first.words, 1210)?.opacity).toBe(0.5);
  const next = reduceOverlayPreview(first, partial({ ordinal: 1, text: 'first next' }), 1210);
  expect(previewTiming.previewCaretState(next.words, 1210)).toEqual({ wordIndex: 1, progress: 0, opacity: 1 });
  const cleared = reduceOverlayPreview(next, partial({ phase: 'cleared', text: '' }));
  expect(previewTiming.previewCaretState(cleared.words, 1210)).toBeNull();
});

test.each(['visible', 'fading'] as const)('cancel or failure clears %s text and rejects late results', (phase) => {
  const visible = reduceOverlayPreview(emptyPreview, partial());
  const current = phase === 'fading' ? reduceOverlayPreview(visible, completed('a')) : visible;
  const cleared = reduceOverlayPreview(current, partial({ text: '', phase: 'cleared' }));
  expect(cleared.text).toBe('');
  expect(reduceOverlayPreview(cleared, partial())).toEqual(cleared);
  expect(reduceOverlayPreview(cleared, completed('a'))).toEqual(cleared);
});

test('28-word chunk at 350 ms keeps the revealed row visible instead of scrolling to hidden rows', () => {
  const preview = reduceOverlayPreview(emptyPreview, partial({
    text: Array.from({ length: 28 }, (_, i) => `word${i}`).join(' ')
  }), 1000);
  const rows = preview.words.map((word, i) => ({
    bottom: (Math.floor(i / 7) + 1) * 17.5,
    revealAt: word.arrivedAt + word.revealDelayMs
  }));
  expect(rows.filter((word) => word.revealAt > 1350)).toHaveLength(16);
  expect(previewContentOffset(rows, 35, 1000)).toBe(8.75);
  expect(previewContentOffset(rows, 35, rows[7].revealAt - 1)).toBe(8.75);
  expect(previewContentOffset(rows, 35, rows[7].revealAt)).toBe(0);
  expect(previewContentOffset(rows, 35, 1350)).toBe(0);
  expect(previewContentOffset(rows, 35, rows[14].revealAt)).toBe(-17.5);
  expect(previewContentOffset(rows, 35, 1900)).toBe(-35);
  expect(previewContentOffset(rows, 35, 1000, true)).toBe(-35);
});

test('short revealed content centres in the 36-pixel clip, including fractional row heights', () => {
  const rows = [
    { bottom: 17.55, revealAt: 1000 },
    { bottom: 35.1, revealAt: 2000 }
  ];
  expect(previewContentOffset(rows, 36, 1000)).toBeCloseTo(9.225);
  expect(previewContentOffset(rows, 36, 2000)).toBeCloseTo(0.45);
  expect(previewContentOffset(rows, 36, 1000, true)).toBeCloseTo(0.45);
});

test('empty or not-yet-revealed preview content has no offset', () => {
  expect(previewContentOffset([], 36, 1000)).toBe(0);
  expect(previewContentOffset([{ bottom: 17.55, revealAt: 2000 }], 36, 1000)).toBe(0);
});

test('third chunk starts on the bottom row and pushes older rows out only as words reveal', () => {
  const first = reduceOverlayPreview(emptyPreview, partial({ text: 'first row' }), 1000);
  const second = reduceOverlayPreview(first, partial({ ordinal: 1, text: 'first row second row' }), 2000);
  const third = reduceOverlayPreview(second, partial({ ordinal: 2, text: 'first row second row third row last row' }), 3000);
  const rows = third.words.map((word, i) => ({
    bottom: (Math.floor(i / 2) + 1) * 17.5,
    revealAt: word.arrivedAt + word.revealDelayMs
  }));
  expect(previewContentOffset(rows, 35, 3000)).toBe(-17.5);
  expect(previewContentOffset(rows, 35, 3119)).toBe(-17.5);
  expect(previewContentOffset(rows, 35, 3120)).toBe(-35);
  expect(previewContentOffset(rows, 35, 5000)).toBe(-35);
  expect(previewContentOffset(rows, 35, 3000, true)).toBe(-35);
});

test.each(['stopping', 'transcribing', 'inserting', 'idle', 'done', 'error'] as const)(
  '%s clears released preview words and caret immediately, once, and blocks late text', (state) => {
    const visible = reduceOverlayPreview(emptyPreview, partial(), 1000);
    const event = { type: 'state_changed', snapshot: { state } } as EchoRuntimeEvent;
    const cleared = reduceOverlayPreview(visible, event, 1010);
    expect(cleared).toMatchObject({ text: '', words: [], phase: 'hidden', cleared: true });
    expect(cleared.generation).toBe(visible.generation + 1);
    expect(previewTiming.previewCaretState(cleared.words, 1010)).toBeNull();
    expect(reduceOverlayPreview(cleared, event)).toBe(cleared);
    expect(reduceOverlayPreview(cleared, partial({ text: '', phase: 'cleared' }))).toBe(cleared);
    expect(reduceOverlayPreview(cleared, partial({ ordinal: 1, text: 'words after release' }))).toBe(cleared);
    expect(reduceOverlayPreview(cleared, completed('a'))).toBe(cleared);
    const next = reduceOverlayPreview(cleared, partial({ dictationId: 'b', ordinal: -1, text: '' }));
    expect(reduceOverlayPreview(next, partial({ dictationId: 'b', text: 'next hold' })).text).toBe('next hold');
  }
);

test('release before eligible words prevents a late tail from opening the pill', () => {
  const waiting = reduceOverlayPreview(emptyPreview, partial({ ordinal: -1, text: '', eligible: false }));
  const released = reduceOverlayPreview(waiting, { type: 'state_changed', snapshot: { state: 'stopping' } } as EchoRuntimeEvent);
  expect(reduceOverlayPreview(released, partial())).toBe(released);
});

// Keep: a caller-owned caret result survives every frame, and live preference settles mounted words.
test('caret reuses supplied output and reduced motion settles an already scheduled word', () => {
  const { words } = reduceOverlayPreview(emptyPreview, partial({ text: 'first words' }), 1000);
  const output = { wordIndex: -1, progress: 0, opacity: 0 };
  expect(previewTiming.previewCaretState(words, 1030, false, output)).toBe(output);
  expect(output).toEqual({ wordIndex: 0, progress: 0.5, opacity: 1 });
  expect(previewTiming.previewCaretState(words, 1282, false, output)).toBe(output);
  expect(output).toEqual({ wordIndex: 1, progress: 1, opacity: 0.5 });
  const animation = previewTiming.previewWordAnimation(words[1], 1050);
  const settled = previewTiming.previewWordAnimation(words[1], 1060, true, animation);
  expect(settled.delayMs).toBeNull();
  expect(previewTiming.previewWordAnimation(words[1], 1070, false, settled)).toBe(settled);
});

// Keep: live preference changes complete the current guarded fade, never a later dictation.
test('controller settles a pending preview fade when reduced motion changes live', async () => {
  const { createOverlayController, initialOverlayState } = await import('../src/lib/ui/overlay/overlay-controller');
  const state = initialOverlayState();
  state.snapshot.state = 'recording';
  let reduced = false;
  const controller = createOverlayController(state, {
    reducedMotion: () => reduced, refreshPreviewWindow() {}, clearTerminalStateTimer() {}, showTerminalStateTemporarily() {}
  });
  try {
    controller.handleRuntimeEvent(partial());
    controller.handleRuntimeEvent(completed('a'));
    expect(state.preview.phase).toBe('fading');
    reduced = true; controller.motionChanged();
    expect(state.preview).toMatchObject({ phase: 'hidden', text: '', words: [] });
    controller.handleRuntimeEvent(partial({ dictationId: 'b', ordinal: -1, text: '', eligible: false }));
    controller.handleRuntimeEvent(partial({ dictationId: 'b', text: 'new words' }));
    controller.motionChanged();
    expect(state.preview.text).toBe('new words');
    controller.dispose();
    controller.handleRuntimeEvent(completed('b'));
    expect(state.preview.text).toBe('new words');
  } finally { controller.dispose(); }
});

// Keep: a queued stale fade cannot drop ownership of a newer timer during disposal.
test('stale preview timers cannot orphan a newer fade timer', async () => {
  const { createOverlayController, initialOverlayState } = await import('../src/lib/ui/overlay/overlay-controller');
  const state = initialOverlayState();
  state.snapshot.state = 'recording';
  const nativeSchedule = globalThis.setTimeout, nativeCancel = globalThis.clearTimeout;
  let id = 0;
  const timers = new Map<number, () => void>();
  (globalThis as any).setTimeout = (fn: () => void) => { timers.set(++id, fn); return id; };
  (globalThis as any).clearTimeout = (key: number) => timers.delete(key);
  const controller = createOverlayController(state, {
    reducedMotion: false, refreshPreviewWindow() {}, clearTerminalStateTimer() {}, showTerminalStateTemporarily() {}
  });
  try {
    controller.handleRuntimeEvent(partial()); controller.handleRuntimeEvent(completed('a'));
    const stale = [...timers.values()][0];
    controller.handleRuntimeEvent(partial({ dictationId: 'b', ordinal: -1, text: '', eligible: false }));
    controller.handleRuntimeEvent(partial({ dictationId: 'b', text: 'new words' }));
    controller.handleRuntimeEvent(completed('b'));
    expect(timers.size).toBe(1);
    stale(); expect(state.preview.text).toBe('new words');
    controller.dispose(); expect(timers.size).toBe(0);
  } finally {
    controller.dispose(); globalThis.setTimeout = nativeSchedule; globalThis.clearTimeout = nativeCancel;
  }
});
