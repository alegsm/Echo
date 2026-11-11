import { expect, test } from 'bun:test';
import type { EchoRuntime, EchoRuntimeEvent } from '../src/lib/core/types';
import { initialOverlayState } from '../src/lib/ui/overlay/overlay-controller';
import { clientHarness } from './helpers/mount-svelte';
import { sourcePath } from './helpers/render-svelte';

// Keep: intact components with only the native window/runtime and browser clock simulated.
async function overlay() {
  const h = await clientHarness(new URL('../src/lib/ui/overlay/OverlayApp.svelte', import.meta.url));
  let listener: (event: EchoRuntimeEvent) => void = () => {};
  let snapshot = initialOverlayState().snapshot;
  (h.window as any).__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: 'overlay' }, currentWebview: { label: 'overlay' } },
    invoke: async () => undefined
  };
  const runtime = {
    getSnapshot: async () => snapshot,
    subscribe(fn: typeof listener) { listener = fn; return () => { listener = () => {}; }; },
    dispose() {}
  } as unknown as EchoRuntime;
  async function settle() { for (let i = 0; i < 20; i++) { await Promise.resolve(); h.flush(); } }
  async function send(event: EchoRuntimeEvent) {
    if (event.type === 'state_changed') snapshot = event.snapshot;
    listener(event); h.flush(); await settle();
  }
  async function state(state: typeof snapshot.state) { await send({ type: 'state_changed', snapshot: { ...snapshot, state } }); }
  function hidden(value: boolean) {
    Object.defineProperty(h.window.document, 'hidden', { configurable: true, value });
    h.window.document.dispatchEvent(new h.window.Event('visibilitychange')); h.flush();
  }
  function frames(from: number, to: number) { for (let now = from; now <= to; now += 16) h.frame(now); }
  h.mount({ createRuntime: () => runtime }); await settle();
  return { ...h, send, state, settle, hidden, framesThrough: frames };
}

test('idle settles without callbacks; hidden and live motion preferences stop and resume one loop', async () => {
  const h = await overlay();
  try {
    h.framesThrough(0, 2000);
    expect(h.frames.size).toBe(0);
    expect([...h.target.querySelectorAll('circle')].every(n => n.getAttribute('cy') === '50')).toBe(true);
    await h.state('recording'); h.framesThrough(2016, 2400);
    expect(h.frames.size).toBe(1);
    const nodes = [...h.target.querySelectorAll('circle')];
    h.hidden(true); expect(h.frames.size).toBe(0);
    h.frame(100000); expect(h.frames.size).toBe(0);
    h.hidden(false); expect(h.frames.size).toBe(1);
    const x = nodes[0].getAttribute('cx'); h.frame(100016);
    expect(nodes[0].getAttribute('cx')).toBe(x);
    h.reduced(true); expect(h.frames.size).toBe(0);
    h.reduced(false); expect(h.frames.size).toBe(1);
    expect([...h.target.querySelectorAll('circle')]).toEqual(nodes);
    await h.unmount(); expect(h.frames.size).toBe(0); expect(h.listeners.size).toBe(0);
    h.hidden(true); h.reduced(true); expect(h.frames.size).toBe(0);
  } finally { await h.cleanup(); }
});

test('expanded recording and warning updates do not schedule visual frames', async () => {
  const h = await overlay();
  try {
    (h.target.querySelector('.matrix-shell') as any).click(); await h.settle();
    await h.send({ type: 'partial_transcript_changed', projection: { dictationId: 'a', ordinal: -1, text: '', eligible: false, phase: 'active' } });
    await h.state('recording');
    await h.send({ type: 'recording_progress', intentId: 'a', sessionId: 's', elapsedMs: 540000, limitMs: 600000 });
    expect(h.target.querySelector('[data-recording-limit]')?.textContent).toBe('Recording stops in 0:59');
    expect(h.frames.size).toBe(0);
    await h.send({ type: 'error', code: 'capture_failed', message: 'Failed' });
    expect(h.frames.size).toBe(0);
    h.elapse(6000); await h.settle();
    expect(h.frames.size).toBe(0);
  } finally { await h.cleanup(); }
});

async function surface() {
  const url = sourcePath('src/lib/ui/overlay/OverlaySurface.svelte');
  const h = await clientHarness(`<script>
    import Surface from '${url}';
    let { initial, finished } = $props();
    let model = $state(initial);
    export function update(patch) { Object.assign(model, patch); }
  </script><Surface {model} onMorphFinished={finished} onCollapse={()=>{}} onCopy={()=>{}} onDragEnd={()=>{}} onDragMove={()=>{}} onDragStart={()=>{}} onStop={()=>{}} onToggle={()=>{}} />`);
  const completions: number[] = [];
  const initial = {
    ...initialOverlayState(), snapshot: initialOverlayState().snapshot,
    actionError: null, activityText: 'Listening', animationSpeed: 3, ariaAlertText: '', ariaStatusText: '', copyStatus: null,
    currentModelText: '', errorLabel: '', expanded: false, haloPose: 'halo', hasVisibleError: false,
    hasVisibleSuccess: false, isBusy: true, isLoading: false, particleCount: 60,
    previewWords: [], ringColors: Array(6).fill('#bdeeff'), statusTone: 'active', documentVisible: true
  };
  const component = h.mount({ initial, finished: (generation: number) => completions.push(generation) }) as { update(patch: object): void };
  function update(patch: object) { component.update(patch); h.flush(); }
  return { ...h, update, completions };
}

test('halo and outline stop at their opacity boundaries and stale morph completions cannot finish a newer leg', async () => {
  const h = await surface();
  try {
    h.frame(0);
    h.update({ shell: { phase: 'outward', generation: 1 } });
    const oldTimers = [...h.timers.values()];
    expect(h.frames.size).toBe(2);
    // Only fire the halo fade deadline, leaving morph completion pending.
    for (const [id, timer] of h.timers) if (timer.delay === 240) { h.timers.delete(id); timer.fn(); }
    h.flush(); expect(h.frames.size).toBe(1);
    h.update({ shell: { phase: 'returning', generation: 2 } });
    expect(h.frames.size).toBe(2);
    for (const timer of oldTimers) timer.fn();
    h.flush(); expect(h.completions).toEqual([]); expect(h.frames.size).toBe(2);
    for (const [id, timer] of h.timers) if (timer.delay === 80) { h.timers.delete(id); timer.fn(); }
    h.flush(); expect(h.frames.size).toBe(1);
    h.update({ documentVisible: false }); await Promise.resolve(); h.flush();
    expect(h.frames.size).toBe(0); expect(h.completions).toEqual([2]);
    await h.unmount(); expect(h.timers.size).toBe(0);
  } finally { await h.cleanup(); }
});

test('preview batches geometry reads, moves the caret by transform, then sleeps and rejects stale observers', async () => {
  const h = await surface();
  let reads = 0;
  const resize: (() => void)[] = [], mutations: (() => void)[] = [];
  h.install('ResizeObserver', class { constructor(fn: () => void) { resize.push(fn); } observe() {} disconnect() {} });
  h.install('MutationObserver', class { constructor(fn: () => void) { mutations.push(fn); } observe() {} disconnect() {} });
  const prototype = h.window.HTMLElement.prototype;
  const original = prototype.getBoundingClientRect;
  prototype.getBoundingClientRect = function() {
    reads++;
    return { left: 0, top: 0, width: 50, height: 18, bottom: 18, right: 50, x: 0, y: 0, toJSON() {} } as any;
  };
  try {
    h.update({ shell: { phase: 'landed', generation: 1 }, previewText: 'first words', previewWords: [
      { id: 'one', text: 'first', arrivedAt: 0, revealDelayMs: 0, revealDurationMs: 60, chunkOrdinal: 0 },
      { id: 'two', text: 'words', arrivedAt: 0, revealDelayMs: 72, revealDurationMs: 60, chunkOrdinal: 0 }
    ] });
    h.frame(0);
    const measured = reads;
    expect(measured).toBeGreaterThan(0);
    const caret = h.target.querySelector('[data-preview-caret]') as any;
    h.frame(30);
    expect(reads).toBe(measured);
    expect(caret.style.transform).toContain('translate');
    expect(caret.style.left).toBe(''); expect(caret.style.top).toBe('');
    resize.forEach(fn => fn()); resize.forEach(fn => fn());
    mutations.forEach(fn => fn());
    expect(reads).toBe(measured);
    h.frame(60); expect(reads).toBeGreaterThan(measured);
    h.update({ documentVisible: false }); expect(h.frames.size).toBe(0);
    h.frame(500);
    h.update({ documentVisible: true });
    expect(h.target.querySelectorAll('.preview-word-settled')).toHaveLength(2);
    h.frame(500);
    // The outline remains active; the caret stops after its final fade.
    expect(h.frames.size).toBe(1);
    h.update({ reducedMotion: true });
    expect(h.frames.size).toBe(0);
    expect(h.target.querySelectorAll('.preview-word-settled')).toHaveLength(2);
    h.update({ previewText: '', previewWords: [], shell: { phase: 'resting', generation: 2 } });
    resize.forEach(fn => fn()); mutations.forEach(fn => fn()); h.frame(600);
    expect(h.target.querySelector('[data-preview-caret]')).toBeNull(); expect(h.frames.size).toBe(0);
  } finally { prototype.getBoundingClientRect = original; await h.cleanup(); }
});

// Keep: warning-only layers share geometry; host renders and traces verify colour and compositing.
test('warning layers share geometry and mount only during warnings', async () => {
  const h = await surface();
  try {
    expect(h.target.querySelector('.orb-halo > .echo-threaded-ring')).not.toBeNull();
    expect(h.target.querySelector('.orb-halo .warning-light')).toBeNull();
    h.update({ statusTone: 'warning', ringColors: Array(6).fill('#ff5b64') });
    h.frame(0); h.frame(16);
    const base = [...h.target.querySelectorAll('.orb-halo .warning-base circle')];
    const bright = [...h.target.querySelectorAll('.orb-halo .warning-light circle')];
    expect(base).toHaveLength(60); expect(bright).toHaveLength(60);
    for (let i = 0; i < base.length; i++) {
      expect(base[i].getAttribute('cx')).toBe(bright[i].getAttribute('cx'));
      expect(base[i].getAttribute('cy')).toBe(bright[i].getAttribute('cy'));
    }
    expect(h.frames.size).toBe(1);
    expect(h.target.querySelector('main')!.style.getPropertyValue('--warning-delay')).toBe('');
    h.update({ reducedMotion: true }); expect(h.frames.size).toBe(0);
    for (const statusTone of ['loading', 'success', 'error']) {
      h.update({ statusTone, reducedMotion: false });
      expect(h.target.querySelector('.orb-halo > .echo-threaded-ring')).not.toBeNull();
      expect(h.target.querySelector('.orb-halo .warning-base')).toBeNull();
      expect(h.target.querySelector('.orb-halo .warning-light')).toBeNull();
    }
  } finally { await h.cleanup(); }
});

// Keep: intact app tone changes must repaint a suppressed halo without any frame.
test.each(['handsFree', 'transcribing'] as const)('reduced motion paints the target colour on transition to %s', async (target) => {
  const h = await overlay();
  try {
    h.reduced(true);
    await h.send({ type: 'partial_transcript_changed', projection: {
      dictationId: 'a', ordinal: -1, text: '', eligible: false, phase: 'active'
    } });
    await h.state('recording');
    if (target === 'transcribing') {
      await h.send({ type: 'recording_progress', intentId: 'a', sessionId: 's', elapsedMs: 540000, limitMs: 600000 });
      expect(h.target.querySelector('main')?.getAttribute('data-tone')).toBe('warning');
      await h.state('transcribing');
    } else {
      await h.send({ type: 'hands_free_changed', active: true });
    }
    const circles = [...h.target.querySelectorAll('.orb-halo .echo-threaded-ring circle')];
    expect(circles).toHaveLength(60);
    expect(circles.every(node => node.getAttribute('fill') === (target === 'handsFree' ? '#ffd18a' : '#c9b8ff'))).toBe(true);
    expect(h.frames.size).toBe(0);
  } finally { await h.cleanup(); }
});

// Keep: preserves halo-before-parent colour sampling when waking from a settled idle loop.
test('first active frame paints the previous colour sample and hidden terminal feedback expires without frames', async () => {
  const h = await overlay();
  try {
    await h.state('recording');
    h.frame(16);
    expect(h.target.querySelector('circle')?.getAttribute('fill')).toBe('#f7f4ef');
    h.frame(32);
    expect(h.target.querySelector('circle')?.getAttribute('fill')).toBe('#f1f3f1');
    const stale = [...h.frames.values()];
    h.hidden(true); expect(h.frames.size).toBe(0);
    stale.forEach(fn => fn(48)); h.flush(); expect(h.frames.size).toBe(0);
    await h.send({ type: 'error', code: 'capture_failed', message: 'Failed' });
    expect(h.target.querySelector('main')?.getAttribute('data-tone')).toBe('error');
    expect(h.frames.size).toBe(0);
    h.elapse(5000);
    expect(h.target.querySelector('main')?.getAttribute('data-tone')).toBe('active');
    expect(h.frames.size).toBe(0);
  } finally { await h.cleanup(); }
});

// Keep: a cancelled terminal timer cannot expire feedback from a newer event.
test('stale terminal timers leave newer feedback visible for its own lifetime', async () => {
  const h = await overlay();
  try {
    h.hidden(true);
    await h.send({ type: 'error', code: 'capture_failed', message: 'First' });
    const old = [...h.timers.values()].find(timer => timer.delay === 4000)!;
    await h.state('recording');
    await h.send({ type: 'error', code: 'capture_failed', message: 'Second' });
    old.fn(); h.flush();
    expect(h.target.querySelector('main')?.getAttribute('data-tone')).toBe('error');
    expect(h.target.querySelector('[role="alert"]')?.textContent).toBe('Second');
  } finally { await h.cleanup(); }
});

// Keep: appending a wrapped row retains earlier geometry; resizing invalidates all rows.
test('preview append remeasures the last wrapped row while earlier rows stay cached', async () => {
  const h = await surface();
  const resize: (() => void)[] = [], mutation: (() => void)[] = [];
  h.install('ResizeObserver', class { constructor(fn: () => void) { resize.push(fn); } observe() {} disconnect() {} });
  h.install('MutationObserver', class { constructor(fn: () => void) { mutation.push(fn); } observe() {} disconnect() {} });
  const prototype = h.window.HTMLElement.prototype;
  const original = prototype.getBoundingClientRect;
  const reads = new Map<string, number>();
  prototype.getBoundingClientRect = function() {
    const id = this.dataset.wordId ?? 'content';
    reads.set(id, (reads.get(id) ?? 0) + 1);
    const top = Number(id) >= 2 ? 18 : 0;
    return { left: 0, top, width: 50, height: 18, bottom: top + 18, right: 50, x: 0, y: top, toJSON() {} } as any;
  };
  const words = Array.from({ length: 5 }, (_, i) => ({ id: String(i), text: 'word', arrivedAt: 0, revealDelayMs: i * 60, revealDurationMs: 48, chunkOrdinal: 0 }));
  try {
    h.update({ shell: { phase: 'landed', generation: 1 }, previewText: 'word word word word', previewWords: words.slice(0, 4) });
    h.frame(0);
    expect(reads.get('0')).toBe(1); expect(reads.get('3')).toBe(1);
    h.update({ previewText: 'word word word word word', previewWords: words });
    mutation.forEach(fn => fn());
    h.frame(16);
    expect(reads.get('0')).toBe(1); expect(reads.get('1')).toBe(1);
    expect(reads.get('2')).toBe(2); expect(reads.get('3')).toBe(2); expect(reads.get('4')).toBe(1);
    resize.forEach(fn => fn()); h.frame(32);
    expect(reads.get('0')).toBe(2); expect(reads.get('4')).toBe(2);
  } finally { prototype.getBoundingClientRect = original; await h.cleanup(); }
});
