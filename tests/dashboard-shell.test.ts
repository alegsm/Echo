import { createHistoryController, initialHistoryState } from '../src/lib/ui/dashboard/panels/history-controller';
import { mountParticleField } from '../src/lib/ui/dashboard/particle-field';
import type { EchoErrorCode, EchoRuntime } from '../src/lib/core/types';
import type { TranscriptRecord } from '../src/lib/core/transcript';
import { createDashboardController, initialDashboardState, errorLabels } from '../src/lib/ui/dashboard/dashboard-controller';
import { beforeAll, expect, test } from 'bun:test';
import { compileRenderer } from './helpers/render-svelte';


function nativeRecord(id: string, text = ''): TranscriptRecord {
  return { id, text, createdAt: '2026-11-06T15:00:00Z', durationMs: 0,
    providerId: 'groq-whisper-turbo', source: 'desktop', insertionMode: 'copied',
    timings: { recordingMs: 0, blobReadyMs: 0, uploadMs: 0, stopToTranscriptMs: 0 },
    audio: { mimeType: 'audio/wav', byteSize: 0 } };
}

// Server rendering proves shell markup; browser events and frost need the host harness.
let render: () => string;
const fixture: {
  history?: unknown[];
  settings?: unknown;
  permissions?: unknown;
  historyLoadError?: string | null;
  historyError?: string;
  historyLoading?: boolean;
  totalCount?: number;
  actionError?: string;
  tab?: string;
} = {};
beforeAll(async () => {
  const compiled = await compileRenderer(new URL('./helpers/DashboardFixture.svelte', import.meta.url));
  render = () => compiled({ initialState: { ...fixture, historyPanel: {
    historyLoadError: fixture.historyLoadError, historyError: fixture.historyError,
    historyLoading: fixture.historyLoading, totalCount: fixture.totalCount
  } } });
}, 60_000);
async function renderDashboard(records?: { id: string; text: string; createdAt: string }[], retention = { days: 30 as number | undefined }, permissions?: unknown) {
  fixture.history = records;
  fixture.settings = { historyRetentionDays: retention.days };
  fixture.permissions = permissions;
  return render();
}

test.each([7, 30, 90, undefined, 365, 42])('History head shows actual retention and checked-state segments for %s days', async (retention) => {
  const html = await renderDashboard([], { days: retention });
  const head = html.match(/<header[^>]*class="page-header\b[^"]*"[^>]*>([\s\S]*?)<\/header>/)?.[1] ?? '';
  expect(head).toContain(`0 transcripts · ${retention === undefined ? 'kept forever' : `kept for ${retention} days`}`);
  expect(head).not.toContain('RECORD');
  expect(head).not.toContain('SAVED');
  const radios = head.match(/<input\b[^>]*type="radio"[^>]*>/g) ?? [];
  expect(radios.length).toBe(4);
  const checked = radios.filter((radio) => /\schecked(?:\s|>|=)/.test(radio));
  expect(checked.length).toBe(retention === 365 || retention === 42 ? 0 : 1);
  if (checked.length) expect(checked[0]).toContain(`value="${retention ?? 'forever'}"`);
  else expect(head).toContain(`Current: ${retention} days`);
  expect(head).toContain('Clear all');
  expect(html).not.toContain('Messages captured from the always-on overlay');
}, 30_000);

test('Latest actions show labels while older rows use named icons', async () => {
  const html = await renderDashboard([{ id: 'a', text: 'Saved', createdAt: '2026-11-06T15:00:00Z' }, { id: 'b', text: 'Older', createdAt: '2026-11-05T15:00:00Z' }]);
  for (const action of ['Copy', 'Delete']) {
    const button = html.match(new RegExp(`<button[^>]*aria-label="${action} transcript[^>]*>[\\s\\S]*?<\\/button>`))?.[0] ?? '';
    expect(button).toContain(`title="${action}"`);
    expect(button).toContain('<svg');
    expect(button).toMatch(new RegExp(`>\\s*${action}\\s*<`));
    const older = html.matchAll(new RegExp(`<button[^>]*aria-label="${action} transcript[^>]*>[\\s\\S]*?<\\/button>`, 'g'));
    expect([...older][1][0]).not.toMatch(new RegExp(`>\\s*${action}\\s*<`));
  }
}, 30_000);

test('History opens as home with only three rail navigation destinations and the Echo mark', async () => {
  const html = await renderDashboard();
  const nav = html.match(/<nav[^>]*aria-label="Echo"[^>]*>([\s\S]*?)<\/nav>/)?.[1] ?? '';
  expect(nav.match(/<button\b/g)?.length).toBe(3);
  expect(nav).toMatch(/<button[^>]*aria-current="page"[^>]*>[\s\S]*?History<\/span><\/button>/);
  for (const label of ['History', 'Phrasebook', 'Settings']) expect(nav).toContain(label);
  const brand = html.match(/<span[^>]*class="dashboard-brand[^"]*"[^>]*>([\s\S]*?)<\/span>/)?.[1] ?? '';
  expect(brand).toContain('<svg');
  expect(html).toMatch(/<h1[^>]*>History<\/h1>/);
}, 30_000);

test('History isolates Latest and groups remaining transcripts by day, escaping transcript text', async () => {
  const html = await renderDashboard([
    { id: 'newest', text: '<script>latest</script>', createdAt: '2026-11-06T15:00:00Z' },
    { id: 'same-day', text: 'Earlier today', createdAt: '2026-11-06T14:00:00Z' },
    { id: 'previous', text: 'Previous day', createdAt: '2026-11-05T14:00:00Z' }
  ]);
  const latest = html.match(/<article[^>]*class="history-latest echo-lit"[^>]*>([\s\S]*?)<\/article>/)?.[1] ?? '';
  expect(latest).toContain('Latest');
  expect(latest).toContain('&lt;script>latest&lt;/script>');
  expect(latest).not.toContain('Earlier today');
  expect(html.match(/data-history-day=/g)?.length).toBe(2);
  expect(html.indexOf('Earlier today')).toBeLessThan(html.indexOf('Previous day'));
  expect(html.match(/&lt;script>latest&lt;\/script>/g)?.length).toBe(1);
  expect(html).not.toContain('Refresh');
}, 30_000);

test('the floating left rail keeps named destinations before status and content', async () => {
  const html = await renderDashboard();
  for (const label of ['History', 'Phrasebook', 'Settings']) {
    expect(html).toContain(`aria-label="${label}" title="${label}"`);
  }
  expect(html.indexOf('aria-label="History"')).toBeLessThan(html.indexOf('class="dashboard-status'));
  expect(html.indexOf('class="dashboard-status')).toBeLessThan(html.indexOf('<main'));
});

function historyPanel(ctx: Parameters<typeof createHistoryController>[0], effects: Partial<Parameters<typeof createHistoryController>[2]> = {}) {
  const state = initialHistoryState();
  const controller = createHistoryController(ctx, state, {
    tick: async () => {}, deleteButtons: () => [], focusHeading() {}, retentionInputs: () => [], ...effects
  });
  return { ...controller, state, error: () => state.historyLoadError };
}

// Keep: rendered heading focusability; deletion focus is covered by the row-removal matrix.
test('History offers a focusable heading when no row actions remain', async () => {
  expect(await renderDashboard()).toMatch(/<h1[^>]*tabindex="-1"[^>]*>History<\/h1>/);
}, 30_000);

test('History controller reloads native history while retaining the expanded page limit', async () => {
  const run = async (runtime: object) => {
    const state = initialDashboardState(); state.runtime = runtime as EchoRuntime; state.historyLimit = 200;
    const ctx = createDashboardController(state, { tick: async () => {}, focusPermissions() {} }).context;
    const panel = historyPanel(ctx);
    await panel.loadHistory();
    await Promise.resolve();
    return { history: state.history, historyLimit: state.historyLimit, actionError: state.actionError };
  };
  let requested = 0;
  const record = nativeRecord('arrived-in-background');
  expect(await run({ listHistory: async (limit: number) => { requested = limit; return [record]; } }))
    .toEqual({ history: [record], historyLimit: 200, actionError: null });
  expect(requested).toBe(200);
});

test('History owns failed loads, retains rows and clears its error after recovery', async () => {
  const run = async (runtime: object) => {
    const state = initialDashboardState({ history: [nativeRecord('saved')] }); state.runtime = runtime as EchoRuntime; state.historyLimit = 200;
    const ctx = createDashboardController(state, { tick: async () => {}, focusPermissions() {} }).context;
    const panel = historyPanel(ctx);
    await panel.loadHistory();
    const failed = { history: state.history, historyLoadError: panel.error(), actionError: state.actionError };
    await panel.loadHistory();
    return { failed, history: state.history, historyLoadError: panel.error(), actionError: state.actionError, historyLimit: state.historyLimit };
  };
  let calls = 0;
  const limits: number[] = [];
  const result = await run({ listHistory: async (limit: number) => {
    limits.push(limit);
    if (++calls === 1) throw new Error('private native detail');
    return [nativeRecord('recovered')];
  } });
  expect(result).toEqual({
    failed: { history: [nativeRecord('saved')], historyLoadError: 'History could not be loaded. Try again.', actionError: null },
    history: [nativeRecord('recovered')], historyLoadError: null, actionError: null, historyLimit: 200
  });
  expect(limits).toEqual([200, 200]);
  try {
    fixture.historyLoadError = result.failed.historyLoadError;
    expect(await renderDashboard()).toContain('History could not be loaded. Try again.');
  } finally {
    fixture.historyLoadError = undefined;
  }
});

// Keep: native events drive reloads; disposal removes the subscription.
function historyLifecycle() {
  let listener: ((event: any) => void) | undefined, loads = 0, failAction = false;
  const record = nativeRecord('saved');
  const runtime = {
    subscribe(fn: typeof listener) { listener = fn; return () => { listener = undefined; }; },
    async deleteTranscript() { if (failAction) throw new Error('Delete failed'); listener?.({ type: 'history_changed', recordId: 'saved' }); return 1; },
    async clearHistory() { listener?.({ type: 'history_changed' }); return 1; },
    async copyTranscript() { throw new Error('private copy detail'); },
    async getHistoryCount() { return 1; }
  } as unknown as EchoRuntime;
  const shell = initialDashboardState({ history: [record] }); shell.runtime = runtime;
  const ctx = { ...createDashboardController(shell, { tick: async () => {}, focusPermissions() {} }).context,
    loadHistory: async () => { loads++; },
    async updateSettings() { listener?.({ type: 'history_changed' }); return shell.settings; }
  };
  const panel = historyPanel(ctx);
  panel.state.pendingDelete = record; panel.state.clearOpen = true;
  const cleanup = panel.mount()!;
  return { ...panel, get loads() { return loads; }, get error() { return panel.state.historyError; },
    emit(event: any) { listener?.(event); }, unsubscribe: cleanup,
    copy: () => panel.copyTranscript(record), focus: () => panel.loadHistory(),
    failDelete() { failAction = true; panel.state.pendingDelete = record; return panel.deleteTranscript(); } };
}

test('History loads on mount and reloads native history changes, then unsubscribes', async () => {
  const panel = historyLifecycle();
  expect(panel.loads).toBe(1);
  panel.emit({ type: 'history_changed' });
  expect(panel.loads).toBe(2);
  panel.unsubscribe();
  panel.emit({ type: 'history_changed' });
  expect(panel.loads).toBe(2);
});

test('History reloads a just-shown dictation and a later deletion to refresh the native total count', () => {
  const panel = historyLifecycle();
  panel.emit({ type: 'transcript_completed', transcript: { id: 'dictation' } });
  panel.emit({ type: 'history_changed', recordId: 'dictation' });
  expect(panel.loads).toBe(2);
  panel.emit({ type: 'history_changed', recordId: 'dictation' });
  expect(panel.loads).toBe(3);
});

test.each(['deleteTranscript', 'clearHistory', 'applyRetention'])('History %s loads once through its native event', async (action) => {
  const panel = historyLifecycle();
  await panel[action](7);
  expect(panel.loads).toBe(2);
});

test.each(['copy', 'failDelete'])('History preserves a %s failure across focus and history reloads', async (action) => {
  const panel = historyLifecycle();
  await panel[action]();
  const expected = action === 'copy' ? "Couldn't copy" : 'Delete failed';
  expect(panel.error).toBe(expected);
  await panel.focus();
  expect(panel.error).toBe(expected);
  panel.emit({ type: 'history_changed' });
  await Promise.resolve();
  expect(panel.error).toBe(expected);
});

test('Globe configuration does not change the three-permission status', async () => {
  const granted = { granted: true };
  const html = await renderDashboard([], { days: 30 }, { microphone: granted, accessibility: granted, input_monitoring: granted });
  expect(html).toContain('Permissions granted');
  expect(html).not.toContain('Permissions need attention');
});

test('navigation icons are decorative and the shell adds no landmark around banner and main', async () => {
  const html = await renderDashboard();
  const nav = html.match(/<nav[^]*?<\/nav>/)?.[0] ?? '';
  expect(nav.match(/<svg/g)?.length).toBe(3);
  expect(html).toMatch(/class="dashboard-brand[^>]*aria-hidden="true"/);
  expect(html).not.toMatch(/class="dashboard-shell"[^>]*role=/);
});

// Merge: preview failures also preserve committed retention without settings writes.
test('copy and retention preview errors show fixed messages without raw details', async () => {
  const fail = async () => { throw new Error('private native detail'); };
  const shell = initialDashboardState();
  shell.runtime = { copyTranscript: fail, previewHistoryRetention: fail } as unknown as EchoRuntime;
  let writes = 0;
  const inputs = [7, 30, 90].map(value => ({ value: String(value), checked: value === 7 }));
  const ctx = { ...createDashboardController(shell, { tick: async () => {}, focusPermissions() {} }).context, async updateSettings() { writes++; return shell.settings; } };
  const panel = historyPanel(ctx, { retentionInputs: () => inputs });
  await panel.copyTranscript(nativeRecord('a'));
  const copyError = panel.state.historyError;
  await panel.selectRetention('7 days');
  expect(inputs.find(input => input.checked)?.value).toBe('30');
  expect(shell.settings.historyRetentionDays).toBe(30);
  expect(writes).toBe(0);
  const result = { historyError: panel.state.historyError, retentionPreview: panel.state.retentionPreview, copyError };
  expect(result).toEqual({ historyError: 'Retention preview could not be loaded. Try again.', retentionPreview: null, copyError: "Couldn't copy" });
});

// Keep: confirmed deletion follows the real controller through row removal and focus restoration.
test.each([0, 1, 2])('deleting row %s focuses its next row, then previous, then heading', async (index) => {
  for (const length of [3, 1]) {
    const records = Array.from({ length }, (_, i) => nativeRecord(String(i)));
    const priorIndex = Math.min(index, length - 1);
    const shell = initialDashboardState({ history: records });
    let focused: string | undefined, heading = false;
    shell.runtime = { async deleteTranscript(id: string) {
      expect(id).toBe(records[priorIndex].id);
      shell.history = shell.history.filter(record => record.id !== id);
      return 1;
    } } as unknown as EchoRuntime;
    const panel = historyPanel(createDashboardController(shell, { tick: async () => {}, focusPermissions() {} }).context, {
      deleteButtons: () => shell.history.map(record => ({ focus: () => { focused = record.id; } })),
      focusHeading: () => { heading = true; }
    });
    panel.state.pendingDelete = records[priorIndex];
    await panel.deleteTranscript();
    if (shell.history.length) {
      expect(focused).toBe(shell.history[Math.min(priorIndex, shell.history.length - 1)].id);
      expect(heading).toBe(false);
    } else expect(heading).toBe(true);
  }
});

test.each([false, true])('retention radio selection follows a settled commit (reject: %s)', async (reject) => {
  const inputs = [7, 30, 90].map((value) => ({ value: String(value), checked: value === 90 }));
  const shell = initialDashboardState();
  shell.runtime = { previewHistoryRetention: async () => ({ deleteCount: 0, retainCount: 0 }) } as unknown as EchoRuntime;
  const ctx = { ...createDashboardController(shell, { tick: async () => {}, focusPermissions() {} }).context, async updateSettings() {
    if (reject) throw new Error('Rejected');
    shell.settings = { ...shell.settings, historyRetentionDays: 90 }; return shell.settings;
  } };
  const panel = historyPanel(ctx, { retentionInputs: () => inputs });
  const pending = panel.selectRetention('90 days');
  expect(inputs.find((input) => input.checked)?.value).toBe('30');
  await pending;
  expect(inputs.find((input) => input.checked)?.value).toBe(reject ? '30' : '90');
});

// Run the component's mount body with a controlled clock and canvas sink.
// Raster output, CSP and keyboard interaction still require the host harness.
async function mountField(reduced = false, hidden = false, focused = true) {
  const events = new Map<string, () => void>();
  const frames = new Map<number, FrameRequestCallback>();
  let nextId = 0, time = 0, draws = 0;
  let points: number[][] = [];
  const context = {
    fillStyle: '', globalAlpha: 1,
    clearRect: () => { draws++; points = []; }, beginPath: () => {}, fill: () => {},
    rect: (...values: number[]) => points.push([...values, context.globalAlpha])
  };
  const canvas = { width: 0, height: 0, style: {}, getContext: () => context };
  const listen = (name: string, fn: () => void) => events.set(name, fn);
  const remove = (name: string) => events.delete(name);
  const motion = { matches: reduced, addEventListener: listen, removeEventListener: remove };
  const window = { innerWidth: 980, innerHeight: 760, devicePixelRatio: 3,
    matchMedia: () => motion, addEventListener: listen, removeEventListener: remove };
  const document = { hasFocus: () => focused, visibilityState: hidden ? 'hidden' : 'visible', addEventListener: listen, removeEventListener: remove };
  const cleanup = mountParticleField(canvas as unknown as HTMLCanvasElement, {
    window: window as unknown as Window, document: document as unknown as Document,
    now: () => time,
    requestFrame: (fn) => { frames.set(++nextId, fn); return nextId; }, cancelFrame: (id) => { frames.delete(id); }
  });
  return { canvas, motion, window, document, events, frames, cleanup: () => cleanup?.(),
    get draws() { return draws; }, get points() { return points; },
    focus(value: boolean) { focused = value; events.get(value ? 'focus' : 'blur')?.(); },
    advance(now: number) { time = now; },
    frame(now: number) { time = now; const pending = [...frames.values()]; frames.clear(); pending.forEach(fn => fn(now)); } };
}

test('the field draws finite points with capped DPR, animation and lifecycle cleanup', async () => {
  expect(await renderDashboard()).toMatch(/<canvas[^>]*aria-hidden="true"/);
  const field = await mountField();
  expect([field.canvas.width, field.canvas.height]).toEqual([980, 760]);
  expect(field.points.length).toBeGreaterThan(1000);
  expect(field.points.every(p => p.every(Number.isFinite))).toBe(true);
  const still = field.points.slice(0, 20);
  expect(field.frames.size).toBe(1);
  field.frame(1000);
  expect(field.points.slice(0, 20)).not.toEqual(still);
  field.window.innerWidth = 320; field.window.devicePixelRatio = 1; field.events.get('resize')?.();
  expect([field.canvas.width, field.canvas.height]).toEqual([320, 760]);
  field.document.visibilityState = 'hidden'; field.events.get('visibilitychange')?.();
  expect(field.frames.size).toBe(0);
  field.document.visibilityState = 'visible'; field.events.get('visibilitychange')?.();
  expect(field.frames.size).toBe(1);
  field.motion.matches = true; field.events.get('change')?.();
  expect(field.frames.size).toBe(0);
  field.window.devicePixelRatio = 3; field.events.get('resize')?.();
  expect(field.canvas.width).toBe(320);
  field.cleanup();
  expect(field.frames.size).toBe(0); expect(field.events.size).toBe(0);
});

test.each([false, true])('reduced motion draws one still field without scheduling frames (hidden: %s)', async (hidden) => {
  const field = await mountField(true, hidden);
  expect(field.draws).toBe(1); expect(field.points.length).toBeGreaterThan(1000);
  expect(field.frames.size).toBe(0);
  field.cleanup();
});

// Keep: a still field must resize even when the recurring motion gates are closed.
test.each(['blurred', 'hidden'] as const)('reduced motion resizes immediately while %s', async (state) => {
  const field = await mountField(true, state === 'hidden', state !== 'blurred');
  try {
    const draws = field.draws;
    field.window.innerWidth = 600;
    field.events.get('resize')?.();
    expect(field.canvas.width).toBe(600);
    expect(field.draws).toBe(draws + 1);
    expect(field.frames.size).toBe(0);
  } finally { field.cleanup(); }
});

// Keep: DPR 1 retains CSS resolution; higher DPR retains the approved cap.
test.each([1, 1.5, 2, 3])('field backing scale and point geometry follow DPR %s', async (dpr) => {
  const field = await mountField(true);
  try {
    const points = field.points;
    field.window.devicePixelRatio = dpr;
    field.events.get('resize')?.();
    expect([field.canvas.width, field.canvas.height]).toEqual([980, 760]);
    const interior = (values: number[][]) => values
      .map(([x, y, size, , alpha]) => [x + size / 2, y + size / 2, size, alpha])
      .filter(([x, y]) => x > 10 && x < 970 && y > 10 && y < 750);
    const before = interior(points), after = interior(field.points);
    expect(after.length).toBe(before.length);
    expect(after.every((point, index) => point.every((value, coordinate) =>
      Math.abs(value - before[index][coordinate] * (coordinate === 2 ? 2 / Math.min(dpr, 2) : 1)) < 0.00001))).toBe(true);
  } finally { field.cleanup(); }
});

// Keep: steady draws do not query window geometry or divide point indices.
test('field steady draws cache window geometry and avoid per-point index rounding', async () => {
  const field = await mountField();
  const floor = Math.floor;
  let rounds = 0, reads = 0;
  try {
    for (const name of ['innerWidth', 'innerHeight', 'devicePixelRatio'] as const) {
      const value = field.window[name];
      Object.defineProperty(field.window, name, { get() { reads++; return value; } });
    }
    Math.floor = (value) => { rounds++; return floor(value); };
    field.frame(100);
    expect(reads).toBe(0);
    expect(rounds).toBeLessThanOrEqual(21_600); // Alpha bucketing alone may round once per point.
  } finally { Math.floor = floor; field.cleanup(); }
});

// Keep: field scheduling is proved with a controlled browser clock, not raster output.
test('field blur and visibility independently gate one loop and defer background resize', async () => {
  const field = await mountField();
  field.frame(100);
  const pose = field.points.slice(0, 20);
  field.focus(false);
  expect(field.frames.size).toBe(0);
  const draws = field.draws;
  field.window.innerWidth = 600;
  field.events.get('resize')?.();
  expect(field.draws).toBe(draws);
  expect(field.canvas.width).toBe(980);
  field.document.visibilityState = 'hidden'; field.events.get('visibilitychange')?.();
  field.focus(true);
  expect(field.frames.size).toBe(0);
  field.focus(false);
  field.document.visibilityState = 'visible'; field.events.get('visibilitychange')?.();
  expect(field.frames.size).toBe(0);
  field.window.innerWidth = 980;
  field.advance(10000);
  for (let i = 0; i < 5; i++) field.focus(true);
  expect(field.frames.size).toBe(1);
  expect(field.points.slice(0, 20)).toEqual(pose);
  field.frame(10034);
  expect(field.points.slice(0, 20)).not.toEqual(pose);
  field.cleanup();
  expect(field.frames.size).toBe(0);
  expect(field.events.size).toBe(0);
});

test('field draws at 30 fps while callbacks follow the display, with no catch-up burst', async () => {
  const field = await mountField();
  const initial = field.draws;
  for (let i = 1; i <= 120; i++) field.frame(i * 1000 / 120);
  expect(field.draws - initial).toBe(30);
  field.frame(5000);
  expect(field.draws - initial).toBe(31);
  expect(field.frames.size).toBe(1);
  field.cleanup();
});

test('live reduced motion resets to pose zero and disabling it in the background stays idle', async () => {
  const field = await mountField();
  const pose = field.points.slice(0, 20);
  field.frame(100);
  field.motion.matches = true; field.events.get('change')?.();
  expect(field.points.slice(0, 20)).toEqual(pose);
  expect(field.frames.size).toBe(0);
  field.focus(false);
  field.motion.matches = false; field.events.get('change')?.();
  expect(field.frames.size).toBe(0);
  field.focus(true);
  expect(field.frames.size).toBe(1);
  field.cleanup();
});

test('unfocused startup schedules no field frames', async () => {
  const field = await mountField(false, false, false);
  expect(field.frames.size).toBe(0);
  field.focus(true);
  expect(field.frames.size).toBe(1);
  field.cleanup();
});

test('Load more renders its current count and becomes a disabled native button while loading', async () => {
  try {
    fixture.totalCount = 201;
    const records = [{ id: 'saved', text: 'Retained row', createdAt: '2026-11-06T15:00:00Z' }];
    for (const pending of [false, true]) {
      fixture.historyLoading = pending;
      const html = await renderDashboard(records);
      const button = html.match(/<button[^>]*>\s*(?:<![^>]*>)*\s*(?:Loading|Load more)[^]*?<\/button>/)?.[0] ?? '';
      expect(button).toContain(pending ? 'Loading' : 'Load more (1 of 201)');
      expect(/\sdisabled(?:\s|>|=)/.test(button)).toBe(pending);
      expect(html).toContain('Retained row');
    }
  } finally {
    fixture.historyLoading = undefined;
    fixture.totalCount = undefined;
  }
});

test.each([false, true])('History controller loadMore requests the next page and settles pending state (failure: %s)', async (failure) => {
  const requests: number[] = [];
  let settle!: () => void;
  const gate = new Promise<void>((resolve) => { settle = resolve; });
  const shell = initialDashboardState({ history: [nativeRecord('saved')] });
  const ctx = { ...createDashboardController(shell, { tick: async () => {}, focusPermissions() {} }).context, loadHistory: async (limit?: number) => {
    requests.push(limit!); await gate;
    if (failure) throw new Error('private load detail');
  } };
  const panel = historyPanel(ctx);
  const pending = panel.loadMore();
  expect(panel.state.historyLoading).toBe(true);
  expect(requests).toEqual([101]);
  settle();
  await pending;
  expect(panel.state.historyLoading).toBe(false);
  expect(panel.state.historyLoadError).toBe(failure ? 'History could not be loaded. Try again.' : null);
  expect(ctx.history()).toEqual([nativeRecord('saved')]);
});

test('empty History retains its prompt inside the existing padded wrapper', async () => {
  const html = await renderDashboard([]);
  expect(html).toMatch(/class="empty-state-wrap"[^>]*>\s*<span[^>]*>Nothing here yet\. Hold your shortcut and speak\.<\/span>/);
  expect(html).not.toContain('Load more (');
});

test.each([
  ['historyError', '<script>synthetic & failure</script>'],
  ['actionError', '<script>synthetic & failure</script>'],
  ['actionError', 'Could not open Settings. Try again.']
] as const)('%s renders %s with an accurate label and assertive alert semantics', async (kind, message) => {
  try {
    fixture[kind] = message;
    const html = await renderDashboard([{ id: 'saved', text: 'Retained row', createdAt: '2026-11-06T15:00:00Z' }]);
    const alert = html.match(/<div[^>]*role="alert"[^>]*>[^]*?class="alert-banner__message[^]*?<\/div>/)?.[0] ?? '';
    expect(alert).toContain('aria-live="assertive"');
    expect(alert).toContain(kind === 'historyError' ? 'History' : 'Action failed');
    expect(alert).toContain(message.replaceAll('&', '&amp;').replaceAll('<', '&lt;'));
    if (kind === 'actionError') expect(alert).not.toContain('Network Failure');
    expect(alert).not.toContain('<script>');
    expect(html).toContain('Retained row');
  } finally { fixture[kind] = undefined; }
});

// Keep: label and message follow the same winning error, then disappear when cleared.
test.each([
  [null, null, undefined, null],
  [null, null, 'missing_provider_key', 'Provider setup'],
  [null, null, 'provider_timeout', 'Network Failure'],
  [null, null, 'provider_failed', 'Network Failure'],
  [null, null, 'capture_failed', 'Action failed'],
  [null, 'Action unavailable', undefined, 'Action failed'],
  [null, 'Action unavailable', 'missing_provider_key', 'Action failed'],
  ['Boot unavailable', null, undefined, 'Network Failure'],
  ['Boot unavailable', null, 'missing_provider_key', 'Network Failure'],
  ['Boot unavailable', 'Action unavailable', undefined, 'Network Failure'],
  ['Boot unavailable', 'Action unavailable', 'missing_provider_key', 'Network Failure']
] satisfies [string | null, string | null, EchoErrorCode | undefined, string | null][])(
  'dashboard alert precedence: boot=%p action=%p snapshot=%p label=%p',
  (bootError, actionError, lastError, label) => {
    const state = initialDashboardState();
    Object.assign(state, { bootError, actionError });
    state.snapshot.lastError = lastError;
    const shell = createDashboardController(state, { tick: async () => {}, focusPermissions() {} });
    expect(shell.alert()).toEqual(label ? {
      label, message: bootError ?? actionError ?? errorLabels[lastError!]
    } : null);
    state.bootError = null;
    state.actionError = null;
    state.snapshot.lastError = undefined;
    expect(shell.alert()).toBeNull();
  }
);

// Cut: shortcut instruction copy is proved by dashboard-settings.test.ts.
// Keep: a native transcript replaces its prior ID projection without duplicating other rows.
test('dashboard runtime events project snapshots, transcripts and fixed error labels', () => {
  const state = initialDashboardState({ history: [nativeRecord('saved', 'Old'), nativeRecord('other', 'Keep')] });
  const shell = createDashboardController(state, { tick: async () => {}, focusPermissions() {} });
  const snapshot = { ...state.snapshot, state: 'recording' as const };
  shell.handleRuntimeEvent({ type: 'state_changed', snapshot });
  const transcript = nativeRecord('saved', 'New');
  shell.handleRuntimeEvent({ type: 'transcript_completed', transcript });
  expect(state.snapshot).toBe(snapshot);
  expect(state.history).toEqual([transcript, nativeRecord('other', 'Keep')]);
  shell.handleRuntimeEvent({ type: 'error', code: 'provider_timeout', message: '' });
  expect(state.actionError).toBe('Provider timed out.');
});

// Keep: mount reads committed state, sets the theme and releases native ownership.
test('dashboard mount hydrates settings and disposes its subscription and runtime', async () => {
  const state = initialDashboardState();
  const shell = createDashboardController(state, { tick: async () => {}, focusPermissions() {} });
  const calls: string[] = [];
  const attributes = new Map<string, string>();
  const saved = { ...state.settings, language: 'es', pushToTalkShortcut: 'Control+K' };
  const runtime = {
    subscribe() { calls.push('subscribe'); return () => { calls.push('unsubscribe'); }; },
    async getSnapshot() { return { ...state.snapshot, state: 'done' }; },
    async getSettings() { return saved; },
    async getDesktopPermissions() { return undefined; },
    async getProviderApiKeyPresence() { calls.push('presence'); return { [saved.providerId]: true }; },
    dispose() { calls.push('dispose'); }
  } as unknown as EchoRuntime;
  const cleanup = shell.mount(() => runtime, {
    setAttribute: (name, value) => { attributes.set(name, value); }
  });
  for (let i = 0; i < 5; i++) await Promise.resolve();
  expect(state.runtime).toBe(runtime);
  expect(state.snapshot.state).toBe('done');
  expect(state.settings).toBe(saved);
  expect(state.hotkeyDraft).toBe('Control+K');
  expect(state.settingsTab).toBe('history');
  expect(calls).toEqual(['subscribe', 'presence']);
  expect(Object.fromEntries(attributes)).toEqual({ 'data-theme': 'chalk', 'data-accent': 'chalk' });
  cleanup();
  expect(calls).toEqual(['subscribe', 'presence', 'unsubscribe', 'dispose']);
});

test.each(['snapshot', 'settings'] as const)('dashboard reports a native %s boot failure without discarding the last snapshot or settings', async (failure) => {
  const state = initialDashboardState();
  const snapshot = state.snapshot, settings = state.settings;
  const shell = createDashboardController(state, { tick: async () => {}, focusPermissions() {} });
  const runtime = {
    subscribe: () => () => {},
    async getSnapshot() { if (failure === 'snapshot') throw new Error('Snapshot unavailable'); return snapshot; },
    async getSettings() { if (failure === 'settings') throw new Error('Settings unavailable'); return settings; },
    async getDesktopPermissions() { return undefined; },
    async getProviderApiKeyPresence() { return {}; },
    dispose() {}
  } as unknown as EchoRuntime;
  const cleanup = shell.mount(() => runtime);
  for (let i = 0; i < 5; i++) await Promise.resolve();
  expect(state.bootError).toBe(failure === 'snapshot' ? 'Snapshot unavailable' : 'Settings unavailable');
  expect(state.snapshot).toBe(snapshot);
  expect(state.settings).toBe(settings);
  cleanup();
});

// Keep: counts come from native authority and fall back to retained rows on failure.
test.each([false, true])('History refreshes the native total count (failure: %s)', async (failure) => {
  const shell = initialDashboardState({ history: [nativeRecord('saved')] });
  shell.runtime = { async getHistoryCount() {
    if (failure) throw new Error('private count detail');
    return 201;
  } } as unknown as EchoRuntime;
  const panel = historyPanel(createDashboardController(shell, { tick: async () => {}, focusPermissions() {} }).context);
  await panel.refreshTotalCount();
  expect(panel.state.totalCount).toBe(failure ? 1 : 201);
});

// Keep: a retention preview never commits the selected radio before confirmation.
test('retention cancellation keeps the committed radio', async () => {
  const shell = initialDashboardState();
  let writes = 0;
  shell.runtime = { async previewHistoryRetention() {
    return { deleteCount: 2, retainCount: 1 };
  } } as unknown as EchoRuntime;
  const inputs = [7, 30, 90].map(value => ({ value: String(value), checked: value === 7 }));
  const ctx = { ...createDashboardController(shell, { tick: async () => {}, focusPermissions() {} }).context, async updateSettings() { writes++; return shell.settings; } };
  const panel = historyPanel(ctx, { retentionInputs: () => inputs });
  await panel.selectRetention('7 days');
  expect(panel.state.retentionPreview).toEqual({ days: 7, label: '7 days', deleteCount: 2, retainCount: 1 });
  panel.state.retentionPreview = null;
  expect(inputs.find(input => input.checked)?.value).toBe('30');
  expect(shell.settings.historyRetentionDays).toBe(30);
  expect(writes).toBe(0);
  expect(panel.state.historyError).toBe(null);
});
