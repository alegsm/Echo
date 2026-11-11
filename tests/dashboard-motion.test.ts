import { expect, test } from 'bun:test';
import { EventTarget, type HTMLButtonElement as TestButton } from 'happy-dom';
import { clientHarness } from './helpers/mount-svelte';
import { defaultSettings } from '../src/lib/core/settings';

// Keep: intact components own their real subscriptions, frames and shine timers.
// Browser clipping, paint and accessibility still require the host proof.
async function motionHarness(component: URL | string) {
  const h = await clientHarness(component);
  h.install('navigator', { platform: 'MacIntel', userAgent: 'test' });
  const callbacks = new Map<number, unknown>();
  let callbackId = 0;
  (h.window as any).__TAURI_INTERNALS__ = {
    transformCallback(fn: unknown) { callbacks.set(++callbackId, fn); return callbackId; },
    unregisterCallback(id: number) { callbacks.delete(id); },
    async invoke(command: string) {
      switch (command) {
        case 'plugin:event|listen': return ++callbackId;
        case 'plugin:event|unlisten': return;
        case 'get_persisted_settings': return { revision: 1, settings: defaultSettings };
        case 'get_desktop_runtime_snapshot': return { sequence: 1, snapshot: { mode: 'desktop', state: 'idle' } };
        case 'list_persisted_history': return { revision: 1, records: [], totalCount: 0 };
        case 'provider_api_key_presence': return { 'groq-whisper-turbo': true };
        case 'get_desktop_permissions': return { platform: 'macos' };
        default: throw new Error(`Unexpected native command: ${command}`);
      }
    }
  };
  (h.window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  h.window.HTMLCanvasElement.prototype.getContext = (() => ({
    clearRect() {}, beginPath() {}, rect() {}, fill() {}, fillStyle: '', globalAlpha: 1
  })) as any;
  h.window.Element.prototype.getAnimations = () => [];
  return h;
}

test('dashboard composes CSS pause, nav shine eligibility and glow intersections, then releases everything', async () => {
  const h = await motionHarness(new URL('../src/lib/ui/dashboard/DashboardApp.svelte', import.meta.url));
  try {
    h.mount(); h.showAll();
    const shell = h.target.querySelector('.dashboard-shell')!;
    const brand = h.target.querySelector('.dashboard-brand')!;
    const tab = h.target.querySelector('[aria-current="page"]')!;
    expect(shell.getAttribute('data-motion-paused')).toBe('false');
    expect(tab.getAttribute('data-motion-visible')).toBe('true');
    expect(h.timers.size).toBe(1);
    h.elapse(5000);
    expect(h.frames.size).toBe(1); // The field also advances the mark.
    h.focus(false);
    expect(shell.getAttribute('data-motion-paused')).toBe('true');
    expect(h.frames.size).toBe(0); expect(h.timers.size).toBe(0);
    h.hidden(true); h.focus(true);
    expect(h.frames.size).toBe(0);
    h.focus(false); h.hidden(false);
    expect(h.frames.size).toBe(0);
    h.reduced(true); h.focus(true);
    expect(h.frames.size).toBe(0); expect(h.timers.size).toBe(0);
    h.reduced(false);
    for (let i = 0; i < 5; i++) { h.focus(false); h.focus(true); }
    expect(h.frames.size).toBe(1); expect(h.timers.size).toBe(1);
    h.intersect(brand as unknown as Element, false);
    expect(h.timers.size).toBe(0);
    h.intersect(tab as unknown as Element, false);
    expect(tab.getAttribute('data-motion-visible')).toBe('false');
    await h.unmount();
    expect(h.frames.size).toBe(0); expect(h.timers.size).toBe(0);
    expect(h.listeners.size).toBe(0); expect(h.subscriptions.size).toBe(0); expect(h.observers.size).toBe(0);
  } finally { await h.cleanup(); }
}, 60_000);

// Merge: checking-light gates and lap parity live in provider-check-light.test.ts.

test('a provider check settles its result while its background decoration stays stopped', async () => {
  let complete!: () => void;
  const probe = new Promise<void>((resolve) => { complete = resolve; });
  const h = await motionHarness(`<script>
    import ProviderPanel from '#lib/ui/dashboard/panels/ProviderPanel.svelte';
    import { provideDashboardContext } from '#lib/ui/dashboard/context.ts';
    import { defaultSettings } from '#lib/core/settings.ts';
    let { probe } = $props();
    let settings = $state({ ...defaultSettings });
    provideDashboardContext({
      settings: () => settings, permissions: () => ({ platform: 'macos' }),
      snapshot: () => ({ state: 'idle' }), runtime: () => ({ probeProviderKey: () => probe }),
      apiKeyDrafts: () => ({ 'groq-whisper-turbo': '' }),
      providerKeyPresence: () => ({ 'groq-whisper-turbo': true }),
      updateSettings: async (patch) => { Object.assign(settings, patch); return settings; },
      refreshProviderKeyPresence: async () => {}, setActionError() {}
    });
  </script><ProviderPanel />`);
  try {
    h.mount({ probe }); h.showAll();
    const button = h.target.querySelector<TestButton>('.check-key')!;
    button.click(); h.settleEvents(); h.showAll();
    expect(button.getAttribute('aria-busy')).toBe('true');
    expect(h.frames.size).toBe(1);
    h.focus(false);
    expect(h.frames.size).toBe(0);
    complete();
    for (let i = 0; i < 10; i++) await Promise.resolve();
    h.flush();
    expect(button.getAttribute('aria-busy')).toBe('false');
    expect(h.target.querySelector('.check-result')!.textContent).toContain('Key works');
    expect(h.frames.size).toBe(0);
    expect(h.target.querySelector('.check-light')).toBe(null);
    await h.unmount();
    expect(h.observers.size).toBe(0); expect(h.listeners.size).toBe(0);
  } finally { await h.cleanup(); }
}, 60_000);

test('standalone mark retains shine by default and cancels its timer when eligibility changes', async () => {
  const h = await motionHarness(`<script>
    import EchoMark from '#lib/ui/brand/EchoMark.svelte';
    let allowed = $state(true);
  </script><button onclick={() => allowed = !allowed}>Toggle</button><EchoMark shinePeriodMs={5000} motionAllowed={allowed} />`);
  try {
    h.mount(); expect(h.timers.size).toBe(1);
    h.elapse(5000); expect(h.frames.size).toBe(1);
    h.target.querySelector('button')!.click(); h.settleEvents();
    expect(h.frames.size).toBe(0); expect(h.timers.size).toBe(0);
    h.target.querySelector('button')!.click(); h.settleEvents();
    expect(h.timers.size).toBe(1);
    await h.unmount();
    expect(h.timers.size).toBe(0); expect(h.frames.size).toBe(0); expect(h.listeners.size).toBe(0);
  } finally { await h.cleanup(); }
}, 60_000);

// Keep: the stylesheet contract pins the approved single-layer glow and its paused clock.
test('glow uses one static shadow and paused opacity from 1 to 0.85 and back', async () => {
  const css = await Bun.file(new URL('../src/lib/ui/dashboard/dashboard-tokens.css', import.meta.url)).text();
  const keyframes = [...css.matchAll(/@keyframes\s+echo-[^{]+\{([\s\S]*?)(?=\n\})/g)];
  expect(keyframes.length).toBe(1);
  expect(keyframes[0][1]).toContain('0%, 100% { opacity: 1; }');
  expect(keyframes[0][1]).toContain('50% { opacity: 0.85; }');
  expect(keyframes[0][1]).not.toContain('box-shadow:');
  expect(css).toMatch(/::before[^}]*box-shadow: var\(--lit\)/);
  expect(css).not.toMatch(/::after[^}]*box-shadow: var\(--lit-dim\)/);
  expect(css).toMatch(/animation: echo-glow[^;]*4.8s ease-in-out infinite paused/);
  expect(css).toContain('[data-motion-paused=');
  expect(css).toContain('[data-motion-visible=');
  expect(css).toContain('animation-play-state: paused !important');
});

// Keep: capture listeners must be removed with the same options used to add them.
test('component harness removes capture listeners with their options', async () => {
  const prototype = EventTarget.prototype as any;
  const original = prototype.removeEventListener;
  const descriptor = Object.getOwnPropertyDescriptor(prototype, 'removeEventListener');
  const removed: unknown[] = [];
  prototype.removeEventListener = function (...args: any[]) {
    if (args[0] === 'focus') removed.push(args[2]);
    return original.apply(this, args);
  };
  let h: Awaited<ReturnType<typeof clientHarness>> | undefined;
  try {
    h = await clientHarness('<span>Listener cleanup</span>');
    let calls = 0;
    const listener = () => { calls++; };
    h.window.addEventListener('focus', listener, { capture: true });
    (h.window as unknown as Window).removeEventListener('focus', listener, { capture: true });
    h.window.dispatchEvent(new h.window.Event('focus'));
    expect(calls).toBe(0);
    expect(removed).toEqual([{ capture: true }]);
  } finally {
    await h?.cleanup();
    if (descriptor) Object.defineProperty(prototype, 'removeEventListener', descriptor);
    else delete prototype.removeEventListener;
  }
});

// Keep: WAAPI is unavailable in happy-dom; observe real component writes through its API boundary.
test('glow and nav shine change only on field draws and release their subscriptions', async () => {
  const h = await motionHarness(new URL('../src/lib/ui/dashboard/DashboardApp.svelte', import.meta.url));
  const writes = new Map<Element, number[]>();
  let queries = 0;
  h.window.Element.prototype.getAnimations = function () {
    queries++;
    const values: number[] = [];
    writes.set(this as unknown as Element, values);
    return [{ animationName: 'echo-glow', set currentTime(value: number) { values.push(value); } }] as any;
  };
  try {
    h.mount(); h.showAll();
    const tab = h.target.querySelector('[aria-current="page"]')!;
    tab.dispatchEvent(new h.window.Event('animationstart'));
    const shine = h.target.querySelector('.dashboard-brand g[stroke-dashoffset]')!;
    const pose = () => shine.getAttribute('stroke-dashoffset');
    h.frame(100);
    const phases = writes.get(tab as unknown as Element) ?? [];
    expect(phases.length).toBeGreaterThan(0);
    expect(phases.at(-1)).toBeCloseTo(100);
    const draws = phases.length, lookups = queries;
    h.frame(100 + 1000 / 60);
    expect(phases.length).toBe(draws);
    expect(queries).toBe(lookups);
    h.frame(100 + 1000 / 30);
    expect(phases.length).toBeGreaterThan(draws);
    expect(phases.at(-1)).toBeCloseTo(100 + 1000 / 30);
    expect(queries).toBe(lookups);
    h.elapse(5000);
    expect(h.frames.size).toBe(1);
    h.frame(5000);
    const start = pose();
    h.frame(5000 + 1000 / 60);
    expect(pose()).toBe(start);
    h.frame(5000 + 1000 / 30);
    expect(pose()).not.toBe(start);
    h.intersect(tab as unknown as Element, false);
    const offscreen = phases.length;
    h.frame(5100);
    expect(phases.length).toBe(offscreen);
    h.focus(false);
    const paused = phases.length;
    h.frame(5200);
    expect(phases.length).toBe(paused);
    h.reduced(true);
    h.frame(5300);
    expect(phases.length).toBe(paused);
    h.focus(true); h.reduced(false);
    h.intersect(tab as unknown as Element, true);
    h.frame(5400);
    await h.unmount();
    expect(h.frames.size).toBe(0); expect(h.timers.size).toBe(0);
    expect(h.listeners.size).toBe(0); expect(h.subscriptions.size).toBe(0); expect(h.observers.size).toBe(0);
    const released = phases.length;
    h.mount(); h.showAll(); h.frame(5500);
    expect(phases.length).toBe(released);
  } finally { await h.cleanup(); }
}, 60_000);
