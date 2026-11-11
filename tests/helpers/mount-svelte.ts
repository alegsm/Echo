import { Window } from 'happy-dom';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { compileClient } from './render-svelte';

// Each caller owns and restores its DOM and deterministic animation clock.
export async function clientHarness(component: URL | string) {
  const window = new Window();
  const saved = new Map<string, PropertyDescriptor | undefined>();
  function install(name: string, value: unknown) {
    saved.set(name, Object.getOwnPropertyDescriptor(globalThis, name));
    Object.defineProperty(globalThis, name, { configurable: true, writable: true, value });
  }
  for (const name of ['window', 'document', 'navigator', 'Node', 'Element', 'HTMLElement', 'HTMLMediaElement', 'HTMLInputElement', 'SVGElement', 'Text', 'Comment', 'Event', 'MouseEvent', 'MutationObserver', 'ResizeObserver', 'getComputedStyle']) {
    install(name, (window as any)[name] ?? (name === 'window' ? window : undefined));
  }
  let time = 0, id = 0;
  const frames = new Map<number, FrameRequestCallback>();
  const timers = new Map<number, { fn: () => void; delay: number }>();
  const listeners = new Set<EventListenerOrEventListenerObject>();
  const motion = {
    matches: false,
    addEventListener(_name: string, fn: EventListenerOrEventListenerObject) { listeners.add(fn); },
    removeEventListener(_name: string, fn: EventListenerOrEventListenerObject) { listeners.delete(fn); }
  };
  window.matchMedia = (() => motion) as any;
  let focused = true, hidden = false;
  Object.defineProperty(window.document, 'hasFocus', { configurable: true, value: () => focused });
  Object.defineProperty(window.document, 'visibilityState', { configurable: true, get: () => hidden ? 'hidden' : 'visible' });
  const observers = new Set<Intersection>();
  class Intersection {
    nodes = new Set<Element>();
    constructor(private callback: IntersectionObserverCallback) { observers.add(this); }
    observe(node: Element) { this.nodes.add(node); }
    disconnect() { this.nodes.clear(); observers.delete(this); }
    notify(node: Element, visible: boolean) {
      if (this.nodes.has(node)) this.callback([{ target: node, isIntersecting: visible } as IntersectionObserverEntry], this as unknown as IntersectionObserver);
    }
  }
  (window as any).IntersectionObserver = Intersection;
  const subscriptions = new Set<EventListenerOrEventListenerObject>();
  for (const [target, names] of [[window, ['focus', 'blur']], [window.document, ['visibilitychange']]] as const) {
    const add = target.addEventListener.bind(target);
    // happy-dom omits the browser API's options argument from its removal type.
    const remove = target.removeEventListener.bind(target) as unknown as EventTarget['removeEventListener'];
    target.addEventListener = ((name: string, listener: EventListenerOrEventListenerObject, options?: any) => {
      if ((names as readonly string[]).includes(name)) subscriptions.add(listener);
      add(name, listener as any, options);
    }) as any;
    target.removeEventListener = ((name: string, listener: EventListenerOrEventListenerObject, options?: any) => {
      if ((names as readonly string[]).includes(name)) subscriptions.delete(listener);
      remove(name, listener, options);
    }) as any;
  }
  install('performance', { now: () => time });
  install('requestAnimationFrame', (fn: FrameRequestCallback) => { frames.set(++id, fn); return id; });
  install('cancelAnimationFrame', (key: number) => frames.delete(key));
  install('setTimeout', (fn: () => void, delay: number) => { timers.set(++id, { fn, delay }); return id; });
  install('clearTimeout', (key: number) => timers.delete(key));
  const restore = () => {
    for (const [name, descriptor] of saved) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else Reflect.deleteProperty(globalThis, name);
    }
  };
  let client;
  try {
    // Register file entries with Bun before the compiler loads their relative imports.
    const entry = component instanceof URL
      ? pathToFileURL(Bun.resolveSync(fileURLToPath(component), import.meta.dir))
      : component;
    client = await compileClient(entry);
  }
  catch (error) {
    try { await window.happyDOM.close(); } finally { restore(); }
    throw error;
  }
  const target = window.document.createElement('div');
  window.document.body.append(target);
  let mounted: object | undefined;
  const flush = () => client.flushSync();
  // Svelte clears event propagation state in a zero-delay task. It is not a shine timer.
  function settleEvents() {
    for (const [id, timer] of timers) if (timer.delay === undefined) { timers.delete(id); timer.fn(); }
    flush();
  }
  return {
    window, target, frames, timers, listeners, observers, subscriptions, flush, install, settleEvents,
    focus(value: boolean) { focused = value; window.dispatchEvent(new window.Event(value ? 'focus' : 'blur')); settleEvents(); },
    hidden(value: boolean) { hidden = value; window.document.dispatchEvent(new window.Event('visibilitychange')); settleEvents(); },
    intersect(node: Element, visible: boolean) { for (const observer of observers) observer.notify(node, visible); flush(); },
    showAll() { for (const observer of observers) for (const node of observer.nodes) observer.notify(node, true); flush(); },
    mount(props: Record<string, unknown> = {}) { mounted = client.mount(client.Component, { target, props }); flush(); return mounted; },
    frame(now: number) { time = now; const callbacks = [...frames.values()]; frames.clear(); callbacks.forEach(fn => fn(now)); flush(); },
    elapse(now: number) { time = now; const callbacks = [...timers.values()]; timers.clear(); callbacks.forEach(timer => timer.fn()); flush(); },
    reduced(value: boolean) { motion.matches = value; for (const fn of listeners) { if (typeof fn === 'function') fn(new window.Event('change') as any); } flush(); },
    async unmount() { if (mounted) { await client.unmount(mounted); mounted = undefined; } flush(); },
    async cleanup() {
      try {
        if (mounted) await client.unmount(mounted);
        await window.happyDOM.close();
      } finally { restore(); }
    }
  };
}
