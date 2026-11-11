import { expect, test } from 'bun:test';
import { compileRenderer } from './helpers/render-svelte';
import { mountParticleHalo, particleTarget, type HaloMotionProps, type ParticlePoint } from '../src/lib/ui/overlay/particle-halo-geometry';
import baseline from './fixtures/particle-halo-baseline.json';

type HaloProps = (typeof baseline.scenarios)[number]['props'];
type Particle = { x: number; y: number; r: number; color: string };
const renderCircles = await compileRenderer(new URL('../src/lib/ui/overlay/HaloCircles.svelte', import.meta.url));
const renderFrame = await compileRenderer(new URL('../src/lib/ui/overlay/ParticleHalo.svelte', import.meta.url));

// Keep: upstream frame parity, including interrupted transitions and frame order,
// through the production mount with a deterministic animation clock.
function mountHalo(initial: HaloProps, reducedMotion: boolean) {
  let props = initial;
  let particles: ParticlePoint[] = [];
  let frameColors: string[] = [];
  let nextId = 0;
  const pending = new Map<number, (now: number) => void>();
  const cancelled: number[] = [];
  const destroy = mountParticleHalo(
    () => props as HaloMotionProps,
    (points, sampledColors) => { particles = points; frameColors = sampledColors; },
    {
      reducedMotion,
      requestFrame(fn) { pending.set(++nextId, fn); return nextId; },
      cancelFrame(id) { cancelled.push(id); pending.delete(id); }
    }
  );
  const snapshot = (): Particle[] => {
    const html = renderCircles({ circles: particles.map((point, i) => ({ ...point, r: 0.9, color: frameColors[i % 6] })) });
    return [...html.matchAll(/<circle\b[^>]*>/g)].map(([circle]) => {
      const attribute = (name: string) => circle.match(new RegExp('\\b' + name + '="([^"]*)"'))?.[1];
      return { x: Number(attribute('cx')), y: Number(attribute('cy')), r: Number(attribute('r')), color: attribute('fill')! };
    });
  };
  return {
    snapshot,
    update(next: HaloProps) { props = next; },
    frame(now: number, next: HaloProps, expectedFrames = reducedMotion || next.pose === 'rest' ? 0 : 1) {
      props = next;
      destroy.update();
      const frames = [...pending.values()];
      pending.clear();
      expect(frames).toHaveLength(expectedFrames);
      frames[0]?.(now);
      return snapshot();
    },
    destroy() {
      const outstanding = [...pending.keys()];
      destroy();
      expect(cancelled).toEqual(expect.arrayContaining(outstanding));
      expect(pending.size).toBe(0);
    }
  };
}

function compare(actual: Particle[], expected: Particle[]) {
  expect(actual).toHaveLength(60);
  for (let i = 0; i < actual.length; i++) {
    expect(Math.abs(actual[i].x - expected[i].x)).toBeLessThanOrEqual(1e-9);
    expect(Math.abs(actual[i].y - expected[i].y)).toBeLessThanOrEqual(1e-9);
    expect(actual[i].r).toBe(expected[i].r);
    expect(actual[i].color).toBe(expected[i].color);
  }
}

for (const scenario of baseline.scenarios) {
  if (scenario.name === 'interruptions-motion') {
    test('interruptions use static rest origins and the last rendered points', () => {
      const halo = mountHalo(scenario.props, false);
      let previous = halo.snapshot();
      let previousTime: number | undefined;
      let geometryTime = 0;
      let pose = scenario.props.pose;
      let origin = previous;
      let start = -Infinity;
      for (const frame of scenario.frames) {
        // A resting loop resets its clock; the fixture's first active frame starts it.
        if (previousTime !== undefined) geometryTime += (frame.now - previousTime) / 1000 * frame.props.animationSpeed;
        if (pose !== frame.props.pose) { origin = previous; start = frame.now; pose = frame.props.pose; }
        const progress = Math.min((frame.now - start) / frame.props.transitionMs, 1);
        const remaining = Math.pow(1 - progress, 3);
        const actual = halo.frame(frame.now, frame.props, frame.now === 0 ? 0 : 1);
        const expected = actual.map((_, i) => {
          const target = particleTarget(pose as HaloMotionProps['pose'], i, 60, geometryTime);
          return { x: target.x + (origin[i].x - target.x) * remaining,
            y: target.y + (origin[i].y - target.y) * remaining, r: 0.9, color: frame.props.colors[i % 6] };
        });
        compare(actual, expected);
        previous = actual;
        if (frame.now !== 0) previousTime = frame.now;
      }
      halo.destroy();
    });
    continue;
  }
  test(`frame parity and teardown with static rest: ${scenario.name}`, () => {
    const halo = mountHalo(scenario.props, scenario.reducedMotion);
    const initial = scenario.reducedMotion ? scenario.frames[0].particles : scenario.initial.map(p => ({ ...p, y: 50 }));
    compare(halo.snapshot(), initial);
    for (const frame of scenario.frames) {
      const expected = frame.props.pose === 'rest' ? frame.particles.map(p => ({ ...p, y: 50 })) : frame.particles;
      compare(halo.frame(frame.now, frame.props), expected);
    }
    halo.destroy();
  });
}

test('halo keeps the upstream decorative SVG viewport and supplied class', () => {
  const html = renderFrame({ ...baseline.scenarios[0].props, class: 'echo-threaded-ring' });
  expect(html).toContain('viewBox="0 0 100 100"');
  expect(html).toContain('width="120" height="120"');
  expect(html).toContain('aria-hidden="true"');
  expect(html).toContain('class="echo-threaded-ring"');
});

test('the derived halo files retain the complete upstream MIT notice', async () => {
  const license = await Bun.file(new URL('../src/lib/ui/overlay/ParticleHalo.LICENSE', import.meta.url)).text();
  expect(new Bun.CryptoHasher('sha256').update(license).digest('hex')).toBe('9d838a3f4cfa25e50f331fd49bd37d38161492af333b0f23a72523b14d9477c6');
  expect(await Bun.file(new URL('../static/licenses/ParticleHalo-MIT.txt', import.meta.url)).text()).toBe(license);
  for (const path of ['ParticleHalo.svelte', 'particle-halo-geometry.ts']) {
    expect(await Bun.file(new URL(`../src/lib/ui/overlay/${path}`, import.meta.url)).text()).toContain('ParticleHalo.LICENSE');
  }
});

// The parent colour tween runs after the halo animation frame.
test('halo paints the previous tween sample when the parent colour tick runs second', () => {
  const fixture = baseline.colorFrameOrder;
  const props = { ...baseline.scenarios[1].props, colors: fixture.initialColors };
  const halo = mountHalo(props, false);
  for (const frame of fixture.frames) {
    halo.frame(frame.now, props);
    props.colors = frame.colorsAfterHaloTick;
    halo.update(props);
    expect(halo.snapshot().map(particle => particle.color)).toEqual(
      Array.from({ length: 60 }, (_, i) => frame.expectedColors[i % 6])
    );
  }
  halo.destroy();
});

// Keep: six threads always have an equal, nonempty number of finite points.
test.each([[0, 6], [1, 6], [7, 6], [59, 60], [60, 60], [61, 60], [65, 66]])(
  'halo normalizes count %s to %s for the six-thread layout', (requested, expected) => {
    const props = { ...baseline.scenarios[0].props, particleCount: requested };
    const halo = mountHalo(props, false);
    for (const particles of [halo.snapshot(), halo.frame(0, props)]) {
      expect(particles).toHaveLength(expected);
      expect(particles.every(point => Number.isFinite(point.x) && Number.isFinite(point.y))).toBe(true);
    }
    halo.destroy();
  }
);

// Keep: detects frame-owned point/colour allocation and suppressed-loop leaks.
test('halo retains output buffers, rests after settling and responds to explicit suppression', () => {
  const props = { ...baseline.scenarios[1].props, eligible: true } as HaloMotionProps;
  let reduced = false;
  const pending = new Map<number, (now: number) => void>();
  let id = 0;
  const outputs: { points: ParticlePoint[]; colors: string[] }[] = [];
  const halo = mountParticleHalo(() => props, (points, colors) => outputs.push({ points, colors }), {
    reducedMotion: () => reduced,
    requestFrame(fn) { pending.set(++id, fn); return id; },
    cancelFrame(key) { pending.delete(key); }
  });
  const frame = (now: number) => { const callbacks = [...pending.values()]; pending.clear(); callbacks.forEach(fn => fn(now)); };
  frame(0); const point = outputs.at(-1)!.points[0];
  frame(16);
  expect(outputs.at(-1)!.points).toBe(outputs[0].points);
  expect(outputs.at(-1)!.colors).toBe(outputs[0].colors);
  expect(outputs.at(-1)!.points[0]).toBe(point);
  props.pose = 'rest'; halo.update(); frame(32); frame(400);
  expect(pending.size).toBe(0);
  expect(outputs.at(-1)!.points.every(p => p.y === 50)).toBe(true);
  props.pose = 'halo'; halo.update(); frame(416);
  const stale = [...pending.values()][0];
  props.eligible = false; halo.update(); expect(pending.size).toBe(0);
  props.eligible = true; halo.update(); expect(pending.size).toBe(1);
  stale(5000); expect(pending.size).toBe(1);
  reduced = true; halo.update(); expect(pending.size).toBe(0);
  reduced = false; halo.update(); expect(pending.size).toBe(1);
  halo.dispose(); expect(pending.size).toBe(0);
});
