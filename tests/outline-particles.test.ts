import { expect, test } from 'bun:test';
import { Window, type Element, type SVGElement, type SVGSVGElement } from 'happy-dom';
import { clientHarness } from './helpers/mount-svelte';
import { compileRenderer, sourcePath } from './helpers/render-svelte';
import * as outline from '../src/lib/ui/overlay/outline-particles';

const perimeter = outline.roundedRectPerimeter(330, 46, 23);
const tailLength = 80 + perimeter * 400 / 2400;
function tailOpacity(behind: number, perimeter: number): number {
  return Math.max(0, 1 - behind / (80 + perimeter * 400 / 2400));
}


test('head walks clockwise from the top-left tangent and resumes the monotonic lap', () => {
  expect(perimeter).toBeCloseTo(568 + 46 * Math.PI);
  for (const [time, fraction] of [[0, 0], [600, 0.25], [1200, 0.5], [1800, 0.75], [2400, 0], [3000, 0.25], [24600, 0.25]]) {
    expect(outline.sweepHeadDistance(time, perimeter)).toBeCloseTo(perimeter * fraction);
  }
});

test('tail has one brightest head and fades continuously to the base over about 200px', () => {
  expect(tailOpacity(0, perimeter)).toBe(1);
  let previous = 1;
  for (let behind = 1; behind <= tailLength; behind++) {
    const opacity = tailOpacity(behind, perimeter);
    expect(opacity).toBeLessThan(previous);
    expect(previous - opacity).toBeLessThanOrEqual(0.06);
    expect(opacity).toBeGreaterThanOrEqual(0);
    previous = opacity;
  }
  expect(tailOpacity(tailLength / 2, perimeter)).toBeCloseTo(0.5);
  expect(tailOpacity(tailLength, perimeter)).toBe(0);
  expect(tailOpacity(perimeter - 1, perimeter)).toBe(0);
});

test('nested light layers composite to the linear tail within a 0.06 step', () => {
  const pieces = outline.sweepLightPieces(perimeter, 128);
  expect(pieces).toHaveLength(128);
  const step = tailLength / pieces.length;
  expect(pieces.at(-1)!.length).toBeCloseTo(tailLength);
  let previous = 1;
  for (let index = 0; index <= pieces.length; index++) {
    const composite = 1 - pieces.slice(index).reduce((transmission, piece) => transmission * (1 - piece.opacity), 1);
    expect(composite).toBeCloseTo(tailOpacity(index * step, perimeter), 12);
    expect(previous - composite).toBeLessThanOrEqual(0.06);
    if (index < pieces.length) {
      expect(pieces[index].length).toBeCloseTo((index + 1) * step);
      const midpoint = (index + 0.5) * step;
      expect(Math.abs(composite - tailOpacity(midpoint, perimeter))).toBeLessThanOrEqual(1 / 128);
    }
    previous = composite;
  }
});

// Keep: explicit eligibility and remounts paint the current lap and own one cancellable loop.
test('one frame loop follows explicit eligibility and motion and releases on detach', async () => {
  const mounted = await clientHarness(`<script>
    import Outline from '${sourcePath('src/lib/ui/overlay/OutlineParticles.svelte')}';
    let visible = $state(true), eligible = $state(false), reducedMotion = $state(false);
    export function show(value) { visible = value; }
    export function allow(value) { eligible = value; }
    export function reduce(value) { reducedMotion = value; }
  </script>{#if visible}<Outline {eligible} {reducedMotion} />{/if}`);
  try {
    const component = mounted.mount() as { show(value: boolean): void; allow(value: boolean): void; reduce(value: boolean): void };
    const read = () => Number(mounted.target.querySelector<SVGSVGElement>('[data-outline-particles]')!.style.getPropertyValue('--sweep-head'));
    expect(mounted.frames.size).toBe(0);
    mounted.elapse(600); component.allow(true); mounted.flush();
    expect(read()).toBeCloseTo(outline.sweepHeadDistance(600, perimeter));
    expect(mounted.frames.size).toBe(1);
    mounted.frame(3600); expect(read()).toBeCloseTo(perimeter / 2);
    const stale = [...mounted.frames.values()][0];
    component.allow(false); mounted.flush(); expect(mounted.frames.size).toBe(0);
    mounted.elapse(3900);
    component.allow(true); mounted.flush(); expect(mounted.frames.size).toBe(1);
    stale(4000); expect(mounted.frames.size).toBe(1);
    expect(read()).toBeCloseTo(outline.sweepHeadDistance(3900, perimeter));
    component.reduce(true); mounted.flush(); expect(mounted.frames.size).toBe(0);
    mounted.elapse(4200);
    component.reduce(false); mounted.flush(); expect(mounted.frames.size).toBe(1);
    expect(read()).toBeCloseTo(outline.sweepHeadDistance(4200, perimeter));
    component.show(false); mounted.flush(); expect(mounted.frames.size).toBe(0);
    mounted.elapse(4500);
    component.show(true); mounted.flush();
    expect(read()).toBeCloseTo(outline.sweepHeadDistance(4500, perimeter));
    expect(mounted.frames.size).toBe(1);
    await mounted.unmount();
    expect(mounted.frames.size).toBe(0); expect(mounted.listeners.size).toBe(0);
    stale(4800); expect(mounted.frames.size).toBe(0);
  } finally { await mounted.cleanup(); }
});

// Keep: unfiltered bloom and core share the same tapered sweep, isolated from the static glow.
test('static outline glow is isolated from the tapered bloom and clean sweep head', async () => {
  const render = await compileRenderer(`<script>import Outline from '${sourcePath('src/lib/ui/overlay/OutlineParticles.svelte')}';</script><Outline /><Outline />`);
  const window = new Window();
  try {
    window.document.body.innerHTML = render();
    const outlines = [...window.document.querySelectorAll('.outline-particles')];
    expect(outlines).toHaveLength(2);
    const ids = outlines.flatMap(root => [...root.querySelectorAll('filter')].map(filter => filter.id));
    expect(ids.length).toBeGreaterThan(0);
    expect(new Set(ids).size).toBe(ids.length);
    const pieces = outline.sweepLightPieces(perimeter, 128);
    for (const root of outlines) {
      const sweep = root.querySelector('.outline-light')!;
      const core = sweep.querySelector('.outline-tail-core')!;
      expect(core).not.toBeNull();
      const paths = [...core.querySelectorAll('rect')];
      expect(paths).toHaveLength(128);
      expect(paths.map(path => path.getAttribute('stroke-dasharray')))
        .toEqual(pieces.map(piece => `${piece.length} ${perimeter - piece.length}`));
      const bloom = [...sweep.querySelectorAll('.outline-tail-bloom')];
      expect(bloom).toHaveLength(2);
      for (const layer of bloom) {
        expect(Number(layer.getAttribute('stroke-width'))).toBeGreaterThan(Number(core.getAttribute('stroke-width')));
        expect(Number(layer.getAttribute('opacity'))).toBeGreaterThan(0);
        expect(Number(layer.getAttribute('opacity'))).toBeLessThan(1);
        const dashes = [...layer.querySelectorAll('rect')];
        expect(dashes.length).toBeGreaterThan(0);
        expect(dashes.at(-1)!.getAttribute('stroke-dasharray')).toBe(paths.at(-1)!.getAttribute('stroke-dasharray'));
        for (const dash of dashes) {
          const length = dash.getAttribute('stroke-dasharray')!.split(' ')[0];
          expect((dash as SVGElement).style.getPropertyValue('stroke-dashoffset')).toBe(`calc(${length} - var(--sweep-head, 0))`);
        }
      }
      expect(sweep.querySelector('[filter]')).toBeNull();
      for (let node: Element | null = sweep; node; node = node.parentElement) {
        expect(node.hasAttribute('filter')).toBe(false);
      }
      const line = root.querySelector('.outline-line')!;
      expect(line.hasAttribute('filter')).toBe(true);
      expect(line.closest('svg')).not.toBe(sweep.closest('svg'));
      const shadows = root.querySelectorAll('feDropShadow');
      expect(shadows).toHaveLength(1);
      expect(root.querySelector('.outline-core')).toBeNull();
      expect(root.querySelector('.outline-head')).toBeNull();
      const local = [...root.querySelectorAll('filter')].map(filter => `url(#${filter.id})`);
      const references = [...root.querySelectorAll('[filter]')].map(node => node.getAttribute('filter'));
      expect(references.length).toBeGreaterThan(0);
      expect(references.every(reference => local.includes(reference!))).toBe(true);
    }
  } finally { await window.happyDOM.close(); }
});
