import { expect, test } from 'bun:test';
import { OverlayWindowShapes, type OverlayPlacement, type OverlaySize } from '../src/lib/ui/overlay/overlay-window-shape';
import { emptyPreview, previewContentOffset, previewWordDelay, reduceOverlayPreview } from '../src/lib/ui/overlay/overlay-preview';
import type { EchoRuntimeEvent } from '../src/lib/core/types';

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

function harness() {
  const requests: unknown[] = [];
  const errors: string[] = [];
  let pause: { step: string; gate: ReturnType<typeof deferred>; entered: ReturnType<typeof deferred> } | undefined;
  let failure: string | undefined;
  async function request(step: string, value?: unknown) {
    requests.push([step, value]);
    if (pause?.step === step) {
      const { gate, entered } = pause;
      pause = undefined;
      entered.resolve();
      await gate.promise;
    }
    if (failure === step) {
      failure = undefined;
      throw new Error(`${step} failed`);
    }
  }
  const shapes = new OverlayWindowShapes({
    onChanged: () => undefined,
    onError: (message) => errors.push(message)
  });
  const native = {
    setSize: (size: OverlaySize) => request('size', size),
    position: (placement: OverlayPlacement) => request('position', placement),
    show: () => request('show')
  };
  return {
    shapes, requests, errors,
    initialize: () => shapes.initialize(native),
    pause(step: string) {
      const gate = deferred();
      const entered = deferred();
      pause = { step, gate, entered };
      return Object.assign(gate.resolve, { entered: entered.promise });
    },
    fail(step: string) { failure = step; }
  };
}

function shapeRequests(width: number, height: number, sourceCellOffset = { x: 0, y: 0 }, keepCell = true) {
  return [
    ['size', { width, height }],
    ['position', { width, height, bottomGap: 0, keepCell, sourceCellOffset }],
    ['show', undefined]
  ];
}

test('user loop: three chunks, timed reveals, expand/collapse, reduced motion, fade and cancel', async () => {
  const h = harness();
  let preview = emptyPreview;
  let now = 1000;
  async function send(event: EchoRuntimeEvent | { type: 'fade_finished'; dictationId: string; generation: number }) {
    preview = reduceOverlayPreview(preview, event, now);
    await h.shapes.setPreview(Boolean(preview.text));
  }
  function partial(phase: 'active' | 'cleared' = 'active', ordinal = 0, text = 'words'): EchoRuntimeEvent {
    return { type: 'partial_transcript_changed', projection: {
      dictationId: 'a', ordinal, text: phase === 'active' ? text : '', eligible: true, phase
    } };
  }
  const complete: EchoRuntimeEvent = { type: 'transcript_completed', dictationId: 'a', transcript: {} as never };
  await h.initialize();
  await send(partial());
  expect(h.shapes.previewReady).toBe(true);
  const firstWords = preview.words;
  expect(previewWordDelay(firstWords[0], now)).toBe(0);
  expect(previewWordDelay(firstWords[0], now + 80)).toBeNull();
  now = 2000;
  await send(partial('active', 1, 'words fill the second row'));
  expect(preview.words[0]).toBe(firstWords[0]);
  expect(preview.words.slice(1).map((word) => word.revealDelayMs)).toEqual([0, 60, 108, 192]);
  now = 3000;
  await send(partial('active', 2, 'words fill the second row and a third chunk pushes older rows away'));
  const thirdWords = preview.words;
  const rows = thirdWords.map((word, i) => ({
    bottom: (Math.floor(i / 5) + 1) * 17.5,
    revealAt: word.arrivedAt + word.revealDelayMs
  }));
  expect(previewContentOffset(rows, 35, now)).toBe(0);
  expect(previewContentOffset(rows, 35, now + 299)).toBe(0);
  expect(previewContentOffset(rows, 35, now + 300)).toBe(-17.5);
  expect(previewContentOffset(rows, 35, now, true)).toBe(-17.5);
  await h.shapes.setExpanded(true);
  expect(h.shapes.previewReady).toBe(false);
  now = 5000;
  await h.shapes.setExpanded(false);
  expect(h.shapes.previewReady).toBe(true);
  expect(preview.words).toBe(thirdWords);
  expect(preview.words.every((word) => previewWordDelay(word, now) === null)).toBe(true);
  expect(previewContentOffset(rows, 35, now)).toBe(-17.5);
  expect(previewWordDelay(thirdWords.at(-1)!, 3000, true)).toBeNull();
  await send(complete);
  expect(preview.words).toBe(thirdWords);
  expect(preview.phase).toBe('fading');
  expect(h.shapes.previewReady).toBe(true);
  expect(h.requests).toHaveLength(12);
  await send({ type: 'fade_finished', dictationId: 'a', generation: preview.generation });
  expect(h.shapes.previewReady).toBe(false);

  preview = emptyPreview;
  await send(partial());
  await h.shapes.setExpanded(true);
  expect(h.shapes.previewReady).toBe(false);
  await h.shapes.setExpanded(false);
  expect(h.shapes.previewReady).toBe(true);
  await send(partial('cleared'));

  preview = emptyPreview;
  const release = h.pause('size');
  const appearing = send(partial());
  await release.entered;
  expect(h.shapes.previewReady).toBe(false);
  const canceling = send(partial('cleared'));
  release();
  await Promise.all([appearing, canceling]);
  expect(h.shapes.previewReady).toBe(false);
  expect(h.requests).toEqual([
    ...shapeRequests(80, 80, { x: 0, y: 0 }, false),
    ...shapeRequests(380, 144),
    ...shapeRequests(380, 210, { x: 150, y: 64 }),
    ...shapeRequests(380, 144, { x: 150, y: 130 }),
    ...shapeRequests(80, 80, { x: 150, y: 64 }),
    ...shapeRequests(380, 144),
    ...shapeRequests(380, 210, { x: 150, y: 64 }),
    ...shapeRequests(380, 144, { x: 150, y: 130 }),
    ...shapeRequests(80, 80, { x: 150, y: 64 }),
    ...shapeRequests(380, 144),
    ...shapeRequests(80, 80, { x: 150, y: 64 })
  ]);
});

test.each(['size', 'position', 'show'])('preview waits for %s and placement completion', async (step) => {
  const h = harness();
  await h.initialize();
  const release = h.pause(step);
  const resizing = h.shapes.setPreview(true);
  await release.entered;
  expect(h.shapes.previewReady).toBe(false);
  release();
  await resizing;
  expect(h.shapes.previewReady).toBe(true);
});

test('latest details request wins while startup or preview resizing is pending', async () => {
  const h = harness();
  const releaseStartup = h.pause('size');
  const startup = h.initialize();
  await releaseStartup.entered;
  const text = h.shapes.setPreview(true);
  const expand = h.shapes.setExpanded(true);
  releaseStartup();
  await Promise.all([startup, text, expand]);
  expect(h.requests).toEqual([...shapeRequests(80, 80, { x: 0, y: 0 }, false), ...shapeRequests(380, 210)]);
  expect(h.shapes.expanded).toBe(true);
  const releaseCollapse = h.pause('position');
  const collapse = h.shapes.setExpanded(false);
  await releaseCollapse.entered;
  const reexpand = h.shapes.setExpanded(true);
  const latestCollapse = h.shapes.setExpanded(false);
  releaseCollapse();
  await Promise.all([collapse, reexpand, latestCollapse]);
  expect(h.requests.slice(6)).toEqual(shapeRequests(380, 144, { x: 150, y: 130 }));
  expect(h.shapes.previewReady).toBe(true);
});

test.each(['size', 'position', 'show'])('%s failure hides preview, reports error, and allows retry', async (step) => {
  const h = harness();
  await h.initialize();
  h.fail(step);
  await h.shapes.setPreview(true);
  expect(h.shapes.previewReady).toBe(false);
  expect(h.errors).toEqual([`${step} failed`]);
  await h.shapes.setPreview(true);
  expect(h.shapes.previewReady).toBe(true);
  expect(h.requests.slice(-3)).toEqual(shapeRequests(380, 144,
    step === 'show' ? { x: 150, y: 64 } : { x: 0, y: 0 }));
});

test.each(['size', 'position', 'show'])('startup %s failure remembers only completed placement', async (step) => {
  const h = harness();
  h.fail(step);
  await h.initialize();
  await h.shapes.setPreview(true);
  expect(h.requests.slice(-3)).toEqual(shapeRequests(380, 144, { x: 0, y: 0 }, step === 'show'));
  expect(h.shapes.previewReady).toBe(true);
});

test.each(['size', 'position', 'show'])('%s failure rolls back expansion, restores the orb, and allows retry', async (step) => {
  const h = harness();
  await h.initialize();
  h.fail(step);
  await h.shapes.setExpanded(true);
  expect(h.shapes.expanded).toBe(false);
  expect(h.requests.slice(-3)).toEqual(shapeRequests(80, 80,
    step === 'show' ? { x: 150, y: 130 } : { x: 0, y: 0 }));
  expect(h.errors).toEqual([`${step} failed`]);
  await h.shapes.setExpanded(true);
  expect(h.shapes.expanded).toBe(true);
  expect(h.requests.slice(-3)).toEqual(shapeRequests(380, 210));
});

test('failed expansion restores retained preview text', async () => {
  const h = harness();
  await h.initialize();
  await h.shapes.setPreview(true);
  h.fail('size');
  await h.shapes.setExpanded(true);
  expect(h.shapes.expanded).toBe(false);
  expect(h.shapes.previewReady).toBe(true);
  expect(h.requests.slice(-3)).toEqual(shapeRequests(380, 144, { x: 150, y: 64 }));
});

test('cancellation during a failing resize still reconciles compact', async () => {
  const h = harness();
  await h.initialize();
  const release = h.pause('size');
  h.fail('size');
  const resizing = h.shapes.setPreview(true);
  await release.entered;
  const canceling = h.shapes.setPreview(false);
  release();
  await Promise.all([resizing, canceling]);
  expect(h.requests.slice(-3)).toEqual(shapeRequests(80, 80));
  expect(h.errors).toEqual(['size failed']);
  expect(h.shapes.previewReady).toBe(false);
});

test('text before native initialization is retained and duplicate text does not resize again', async () => {
  const h = harness();
  await h.shapes.setPreview(true);
  expect(h.shapes.previewReady).toBe(false);
  await h.initialize();
  await h.shapes.setPreview(true);
  expect(h.requests).toEqual(shapeRequests(380, 144, { x: 0, y: 0 }, false));
  expect(h.shapes.previewReady).toBe(true);
});

test('a shape change immediately after a settled no-op still resizes', async () => {
  const h = harness();
  await h.initialize();
  const noop = h.shapes.setPreview(false);
  const text = h.shapes.setPreview(true);
  await Promise.all([noop, text]);
  expect(h.requests).toEqual([...shapeRequests(80, 80, { x: 0, y: 0 }, false), ...shapeRequests(380, 144)]);
  expect(h.shapes.previewReady).toBe(true);
});

test('a request queued at geometry commit is reconciled before its promise settles', async () => {
  const sizes: OverlaySize[] = [];
  let latest: Promise<void> | undefined;
  const shapes = new OverlayWindowShapes({
    onError: (message) => { throw new Error(message); },
    onChanged: ({ previewReady }) => {
      if (previewReady) queueMicrotask(() => { latest = shapes.setExpanded(true); });
    }
  });
  await shapes.initialize({
    async setSize(size) { sizes.push(size); },
    async position() {},
    async show() {}
  });
  await shapes.setPreview(true);
  await latest;
  expect(sizes).toEqual([
    { width: 80, height: 80 }, { width: 380, height: 144 }, { width: 380, height: 210 }
  ]);
});

test('synchronous native failure releases the worker so the next request can retry', async () => {
  const errors: string[] = [];
  const sizes: OverlaySize[] = [];
  let fail = true;
  const shapes = new OverlayWindowShapes({
    onChanged: () => undefined,
    onError: (message) => errors.push(message)
  });
  await shapes.setPreview(true);
  await shapes.initialize({
    setSize(size) {
      if (fail) throw new Error('synchronous resize failure');
      sizes.push(size);
      return Promise.resolve();
    },
    async position() {},
    async show() {}
  });
  expect(shapes.previewReady).toBe(false);
  fail = false;
  await shapes.setPreview(true);
  expect(errors).toEqual(['synchronous resize failure']);
  expect(sizes).toEqual([{ width: 380, height: 144 }]);
  expect(shapes.previewReady).toBe(true);
});
