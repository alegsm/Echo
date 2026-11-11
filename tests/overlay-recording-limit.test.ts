import { expect, test } from 'bun:test';
import type { HTMLButtonElement } from 'happy-dom';
import type { EchoRuntime, EchoRuntimeEvent } from '../src/lib/core/types';
import { createPartialTranscriptState } from '../src/lib/runtime/desktop/partial-transcript-state';
import { initialOverlayState } from '../src/lib/ui/overlay/overlay-controller';
import { clientHarness } from './helpers/mount-svelte';

async function recordingOverlay(reducedMotion = false) {
  const h = await clientHarness(new URL('../src/lib/ui/overlay/OverlayApp.svelte', import.meta.url));
  h.reduced(reducedMotion);
  let snapshot = initialOverlayState().snapshot;
  let listener: (event: EchoRuntimeEvent) => void = () => undefined;
  const requests: { command: string; args: any }[] = [];
  // Only the unavailable Tauri window and native runtime are simulated.
  (h.window as any).__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: 'overlay' }, currentWebview: { label: 'overlay' } },
    invoke: async (command: string, args: unknown) => { requests.push({ command, args }); }
  };
  const runtime = {
    getSnapshot: async () => snapshot,
    subscribe(fn: typeof listener) { listener = fn; return () => undefined; },
    dispose() {}
  } as unknown as EchoRuntime;
  async function settle() {
    for (let i = 0; i < 20; i++) { await Promise.resolve(); h.flush(); }
  }
  async function send(event: EchoRuntimeEvent) {
    if (event.type === 'state_changed') snapshot = event.snapshot;
    listener(event);
    h.flush();
    await settle();
  }
  async function state(state: typeof snapshot.state) {
    await send({ type: 'state_changed', snapshot: { ...snapshot, state } });
  }
  async function begin(intentId = 'a') {
    await send({ type: 'partial_transcript_changed', projection: {
      dictationId: intentId, ordinal: -1, text: '', eligible: false, phase: 'active'
    } });
    await state('requesting_microphone');
    await state('recording');
  }
  async function progress(elapsedMs: number, intentId = 'a', sessionId = `session-${intentId}`) {
    await send({ type: 'recording_progress', intentId, sessionId, elapsedMs, limitMs: 600_000 });
  }
  const warning = () => h.target.querySelector('[data-recording-limit]')?.textContent?.trim();
  const announcement = () => h.target.querySelector('[data-warning-announcement]')?.textContent?.trim() ?? '';
  h.mount({ createRuntime: () => runtime });
  await settle();
  return { ...h, send, state, begin, progress, requests, warning, announcement };
}

// Keep: exercises the intact overlay, native progress stream and serialized window adapter.
test('last minute follows native seconds, keeps one announcement and restores the compact orb cell', async () => {
  const h = await recordingOverlay();
  try {
    await h.begin();
    await h.progress(539_999);
    expect(h.warning()).toBeUndefined();
    expect(h.announcement()).toBe('');
    await h.progress(540_000);
    expect(h.warning()).toBe('Recording stops in 0:59');
    expect(h.target.querySelector('main')?.getAttribute('data-tone')).toBe('warning');
    expect(h.target.querySelector('main')?.classList.contains('has-preview')).toBe(true);
    const announcement = h.announcement();
    expect(announcement).toBe('Recording stops in one minute.');
    const geometry = h.requests.filter(({ command }) => command === 'position_overlay');
    expect(geometry.at(-1)?.args).toMatchObject({ width: 380, height: 144, sourceCellOffset: { x: 0, y: 0 }, keepCell: true });
    // Delayed browser timers and frames must never advance the native countdown.
    h.elapse(570_000);
    h.frame(570_000);
    expect(h.warning()).toBe('Recording stops in 0:59');
    for (let second = 541; second <= 599; second++) {
      await h.progress(second * 1000);
      expect(h.warning()).toBe(`Recording stops in 0:${String(599 - second).padStart(2, '0')}`);
      expect(h.announcement()).toBe(announcement);
    }
    await h.progress(599_999);
    expect(h.warning()).toBe('Recording stops in 0:00');
    await h.progress(600_100);
    expect(h.warning()).toBe('Recording stops in 0:00');
    await h.state('stopping');
    expect(h.warning()).toBeUndefined();
    expect(h.announcement()).toBe('');
    expect(h.target.querySelector('main')?.getAttribute('data-tone')).toBe('loading');
    expect(h.requests.filter(({ command }) => command === 'position_overlay').at(-1)?.args)
      .toMatchObject({ width: 80, height: 80, sourceCellOffset: { x: 150, y: 64 }, keepCell: true });
    await h.progress(599_000);
    expect(h.warning()).toBeUndefined();
  } finally { await h.cleanup(); }
});

test('preview text and expanded details coexist with a silent countdown', async () => {
  const h = await recordingOverlay(true);
  try {
    await h.begin();
    await h.send({ type: 'hands_free_changed', active: true });
    await h.send({ type: 'partial_transcript_changed', projection: {
      dictationId: 'a', ordinal: 0, text: 'The live preview remains readable', eligible: true, phase: 'active'
    } });
    await h.progress(540_250);
    expect(h.warning()).toBe('Recording stops in 0:59');
    expect(h.target.querySelector('[data-preview-pill]')?.textContent).toContain('The live preview remains readable');
    expect(h.target.querySelector('[data-recording-limit]')?.closest('[aria-live]')).toBeNull();
    expect(Boolean(h.target.querySelector('[data-recording-limit]')?.closest('[aria-hidden="true"]'))).toBe(false);
    h.target.querySelector<HTMLButtonElement>('.matrix-shell')!.click();
    await h.progress(541_000);
    expect(h.target.querySelector('main')?.classList.contains('expanded')).toBe(true);
    expect(h.warning()).toBe('Recording stops in 0:58');
    expect(h.target.querySelector('.detail')?.textContent).toContain('Listening');
    h.target.querySelector<HTMLButtonElement>('.collapse-button')!.click();
    await h.progress(542_000);
    expect(h.target.querySelector('[data-preview-pill]')?.textContent).toContain('The live preview remains readable');
    expect(h.warning()).toBe('Recording stops in 0:57');
  } finally { await h.cleanup(); }
});

test('preview clear keeps the countdown advancing; cancel and new intents reject stale progress', async () => {
  const h = await recordingOverlay(true);
  try {
    await h.begin();
    await h.progress(541_000);
    expect(h.warning()).toBe('Recording stops in 0:58');
    await h.progress(540_000);
    await h.progress(590_000, 'old');
    await h.progress(590_000, 'a', 'old-session');
    expect(h.warning()).toBe('Recording stops in 0:58');
    await h.send({ type: 'partial_transcript_changed', projection: {
      dictationId: 'a', ordinal: -1, text: '', eligible: false, phase: 'cleared'
    } });
    expect(h.warning()).toBe('Recording stops in 0:58');
    await h.progress(550_000);
    expect(h.warning()).toBe('Recording stops in 0:49');
    await h.state('idle');
    await h.begin('b');
    await h.progress(540_000, 'b');
    expect(h.warning()).toBe('Recording stops in 0:59');
    await h.begin('c');
    expect(h.warning()).toBeUndefined();
    expect(h.announcement()).toBe('');
    await h.progress(599_000, 'b');
    expect(h.warning()).toBeUndefined();
    await h.progress(540_000, 'c');
    expect(h.announcement()).toBe('Recording stops in one minute.');
  } finally { await h.cleanup(); }
});

// Keep: native retry/gap events hide text briefly; later partials never extend the notice.
test('retry and gap notices expire while later partials and the countdown continue', async () => {
  const h = await recordingOverlay(true);
  const partial = createPartialTranscriptState();
  let now = 0, timerId = 0;
  const timers = new Map<number, { due: number; run: () => void }>();
  const schedule = globalThis.setTimeout, cancel = globalThis.clearTimeout;
  (globalThis as any).setTimeout = (run: () => void, delay = 0) => {
    timers.set(++timerId, { due: now + delay, run }); return timerId;
  };
  (globalThis as any).clearTimeout = (id: number) => timers.delete(id);
  async function advance(time: number) {
    now = time;
    for (const [id, timer] of [...timers]) {
      if (timer.due <= now) { timers.delete(id); timer.run(); }
    }
    h.frame(now);
    await Promise.resolve(); h.flush();
  }
  async function project() {
    await h.send({ type: 'partial_transcript_changed', projection: partial.getProjection()! });
  }
  try {
    partial.begin('a');
    await h.begin();
    partial.apply({ dictationId: 'a', ordinal: 0, text: 'saved preview words' });
    partial.apply({ dictationId: 'a', ordinal: 1, text: '', status: 'retrying' });
    await project();
    await h.progress(540_000);
    expect(h.target.querySelector('[data-transcription-notice]')?.textContent).toContain('Retrying');
    await advance(3_000);
    partial.apply({ dictationId: 'a', ordinal: 1, text: '', status: 'gap' });
    await project();
    expect(h.target.querySelector('[data-transcription-notice]')?.textContent).toContain('Transcription gap');
    await advance(4_000);
    partial.apply({ dictationId: 'a', ordinal: 1, text: '[Transcription gap]' });
    partial.apply({ dictationId: 'a', ordinal: 2, text: 'later speech' });
    await project();
    await advance(6_999);
    expect(h.target.querySelector('[data-transcription-notice]')).not.toBeNull();
    await advance(7_000);
    expect(h.target.querySelector('[data-transcription-notice]')).toBeNull();
    expect(h.target.querySelector('[data-preview-pill]')?.textContent).toContain('later speech');
    await h.progress(547_000);
    expect(h.warning()).toBe('Recording stops in 0:52');
    partial.apply({ dictationId: 'a', ordinal: 3, text: 'still recording' });
    await project();
    expect(h.target.querySelector('[data-preview-pill]')?.textContent).toContain('still recording');
    expect(h.target.querySelector('[data-transcription-notice]')).toBeNull();

    // Another piece retrying and recovering never cuts a gap notice short.
    partial.apply({ dictationId: 'a', ordinal: 4, text: '', status: 'gap' });
    await project();
    await advance(8_000);
    partial.apply({ dictationId: 'a', ordinal: 5, text: '', status: 'retrying' });
    await project();
    await advance(8_100);
    partial.apply({ dictationId: 'a', ordinal: 5, text: '', status: 'recovered' });
    await project();
    await advance(8_200);
    expect(h.target.querySelector('[data-transcription-notice]')?.textContent).toContain('Transcription gap');
    await advance(10_999);
    expect(h.target.querySelector('[data-transcription-notice]')?.textContent).toContain('Transcription gap');
    await advance(11_100);
    expect(h.target.querySelector('[data-transcription-notice]')).toBeNull();
    // A recovery clears only its own piece's retry notice.
    partial.apply({ dictationId: 'a', ordinal: 6, text: '', status: 'retrying' });
    await project();
    partial.apply({ dictationId: 'a', ordinal: 7, text: '', status: 'recovered' });
    await project();
    expect(h.target.querySelector('[data-transcription-notice]')?.textContent).toContain('Retrying');
    partial.apply({ dictationId: 'a', ordinal: 6, text: '', status: 'recovered' });
    await project();
    expect(h.target.querySelector('[data-transcription-notice]')).toBeNull();
  } finally {
    partial.dispose();
    globalThis.setTimeout = schedule; globalThis.clearTimeout = cancel;
    await h.cleanup();
  }
});

// Keep: live motion suppression never suppresses the native countdown or its announcement.
test('warning follows live reduced motion and visibility while native seconds remain readable', async () => {
  const h = await recordingOverlay();
  try {
    await h.begin(); await h.progress(540_000);
    h.reduced(true);
    expect(h.frames.size).toBe(0);
    expect(h.target.querySelector('main')?.getAttribute('data-reduced-motion')).toBe('true');
    await h.progress(541_000);
    expect(h.warning()).toBe('Recording stops in 0:58');
    expect(h.announcement()).toBe('Recording stops in one minute.');
    h.reduced(false); expect(h.frames.size).toBe(1);
    Object.defineProperty(h.window.document, 'hidden', { configurable: true, value: true });
    h.window.document.dispatchEvent(new h.window.Event('visibilitychange')); h.flush();
    expect(h.frames.size).toBe(0);
    await h.progress(542_000); expect(h.warning()).toBe('Recording stops in 0:57');
    expect(h.frames.size).toBe(0);
  } finally { await h.cleanup(); }
});
