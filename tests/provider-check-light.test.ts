import { expect, test } from 'bun:test';
import { clientHarness } from './helpers/mount-svelte';

// Keep: the intact light shares the outline lap and owns all motion subscriptions.
test('Check key light follows the shared lap, obeys motion gates and cleans up', async () => {
  const light = await clientHarness(new URL('../src/lib/ui/dashboard/panels/ProviderCheckLight.svelte', import.meta.url));
  try {
    // happy-dom has no layout engine; supply the measured button dimensions.
    Object.defineProperty(light.window.HTMLElement.prototype, 'clientWidth', { get: () => 120 });
    Object.defineProperty(light.window.HTMLElement.prototype, 'clientHeight', { get: () => 34 });
    light.focus(false); light.reduced(true); light.mount();
    const svg = light.target.querySelector('svg')!;
    const read = () => Number(light.target.querySelector('svg')!.style.getPropertyValue('--sweep-head'));
    expect(light.frames.size).toBe(0);
    light.reduced(false);
    expect(light.frames.size).toBe(0);
    light.focus(true);
    expect(light.frames.size).toBe(0);
    light.intersect(svg as unknown as Element, true);
    expect(light.frames.size).toBe(1);
    light.frame(1200); const halfLap = read();
    expect(halfLap).toBeGreaterThan(0);
    light.frame(3600); expect(read()).toBeCloseTo(halfLap);
    light.focus(false); expect(light.frames.size).toBe(0);
    const stopped = read();
    light.frame(4000); expect(read()).toBe(stopped);
    light.hidden(true); light.focus(true); expect(light.frames.size).toBe(0);
    light.hidden(false); expect(light.frames.size).toBe(1);
    light.hidden(true); expect(light.frames.size).toBe(0);
    light.focus(false); light.hidden(false); expect(light.frames.size).toBe(0);
    light.focus(true); expect(light.frames.size).toBe(1);
    light.intersect(svg as unknown as Element, false); expect(light.frames.size).toBe(0);
    light.intersect(svg as unknown as Element, true); expect(light.frames.size).toBe(1);
    light.reduced(true); expect(light.frames.size).toBe(0);
    light.focus(false); light.reduced(false); expect(light.frames.size).toBe(0);
    light.focus(true);
    for (let i = 0; i < 5; i++) { light.focus(false); light.focus(true); }
    expect(light.frames.size).toBe(1);
    expect(light.observers.size).toBe(1);
    expect(light.subscriptions.size).toBe(3);
    await light.unmount();
    expect(light.frames.size).toBe(0); expect(light.listeners.size).toBe(0);
    expect(light.observers.size).toBe(0); expect(light.subscriptions.size).toBe(0);
    light.focus(false); light.focus(true); light.hidden(true); light.hidden(false);
    light.reduced(true); light.reduced(false); light.intersect(svg as unknown as Element, true);
    expect(light.frames.size).toBe(0);
  } finally { await light.cleanup(); }
});
