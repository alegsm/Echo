// @ts-nocheck
import { expect, test } from 'bun:test';
import { createHotkeyController } from '../src/lib/runtime/desktop/hotkey-controller';

// Keep: Fn capture remains available across shortcut reconfiguration.
test('Fn capture starts and survives shortcut reconfiguration', async () => {
  const harness = createHarness();
  const controller = createHotkeyController(harness.options);
  await controller.start();
  await controller.configure('Fn');
  await controller.configure('Control+K');
  await controller.configure('Fn');
  expect(controller.activeShortcut).toBe('Fn');
  expect(harness.fnEnabled).toContain(true);
  await controller.dispose();
});

for (const source of ['Fn', 'Control+Windows', 'Primary', 'Fallback']) {
  test(`${source}: tap latches and the next tap or hold sends once`, async () => {
    for (const finishDuration of [10, 600]) {
      const harness = createHarness({ failShortcut: source === 'Fallback' ? 'Primary' : undefined });
      const controller = createHotkeyController(harness.options);
      await controller.start();
      await controller.configure(source === 'Fallback' ? 'Primary' : source, 'Fallback');
      const edge = (pressed) => ['Fn', 'Control+Windows'].includes(source)
        ? harness.fnListener({ state: pressed ? 'pressed' : 'released' })
        : harness.shortcutListener({ state: pressed ? 'Pressed' : 'Released' });
      edge(true);
      edge(true);
      expect(harness.actions).toEqual(['press']);
      harness.clock.advance(300);
      edge(false);
      harness.clock.advance(79);
      expect(harness.modes).toEqual([]);
      harness.clock.advance(1);
      expect(harness.modes).toEqual([true]);
      expect(harness.actions).toEqual(['press']);
      edge(true);
      harness.clock.advance(finishDuration);
      edge(false);
      harness.clock.advance(79);
      expect(harness.actions).toEqual(['press']);
      harness.clock.advance(1);
      edge(false);
      harness.clock.advance(100);
      expect(harness.actions).toEqual(['press', 'release']);
      expect(harness.modes).toEqual([true, false]);
      await controller.dispose();
    }
  });
}

for (const duration of [0, 299, 300, 301, 1000]) {
  test(`physical release at ${duration} ms is classified after exactly 80 ms`, async () => {
    const harness = createHarness();
    const controller = createHotkeyController(harness.options);
    await controller.start();
    await controller.configure('Fn');
    harness.fnListener({ state: 'pressed' });
    harness.clock.advance(duration);
    harness.fnListener({ state: 'released' });
    harness.clock.advance(79);
    harness.fnListener({ state: 'released' });
    expect(harness.actions).toEqual(['press']);
    harness.clock.advance(1);
    expect(harness.actions).toEqual(duration <= 300 ? ['press'] : ['press', 'release']);
    expect(harness.modes).toEqual([duration <= 300]);
    if (duration > 300) expect(harness.actionTimes.at(-1)).toBe(duration + 80);
    await controller.dispose();
  });
}

test('bounce preserves the original press time during initial and finishing holds', async () => {
  for (const finishing of [false, true]) {
    const harness = createHarness();
    const controller = createHotkeyController(harness.options);
    await controller.start();
    await controller.configure('Fn');
    harness.fnListener({ state: 'pressed' });
    if (finishing) {
      harness.fnListener({ state: 'released' });
      harness.clock.advance(80);
      harness.fnListener({ state: 'pressed' });
    }
    harness.clock.advance(250);
    harness.fnListener({ state: 'released' });
    harness.clock.advance(79);
    harness.fnListener({ state: 'pressed' });
    harness.fnListener({ state: 'pressed' });
    harness.clock.advance(1);
    expect(harness.actions).toEqual(['press']);
    harness.clock.advance(10);
    harness.fnListener({ state: 'released' });
    harness.clock.advance(80);
    expect(harness.actions).toEqual(['press', 'release']);
    expect(harness.modes).toEqual(finishing ? [true, false] : [false]);
    await controller.dispose();
  }
});

test('a press at the debounce deadline is a fresh finishing press even if the timer is delayed', async () => {
  const harness = createHarness();
  const controller = createHotkeyController(harness.options);
  await controller.start();
  await controller.configure('Fn');
  harness.fnListener({ state: 'pressed' });
  harness.fnListener({ state: 'released' });
  harness.clock.elapse(80);
  harness.fnListener({ state: 'pressed' });
  expect(harness.modes).toEqual([true]);
  harness.fnListener({ state: 'released' });
  harness.clock.advance(80);
  expect(harness.actions).toEqual(['press', 'release']);
  await controller.dispose();
});

for (const duration of [100, 1000]) {
  test(`a release at ${duration} ms settles when its timer clock is 0.1 ms early`, async () => {
    const harness = createHarness({ timerSkewMs: 0.1 });
    const controller = createHotkeyController(harness.options);
    await controller.start();
    await controller.configure('Fn');
    harness.fnListener({ state: 'pressed' });
    harness.clock.advance(duration);
    harness.fnListener({ state: 'released' });
    harness.clock.advance(10_000);
    expect(harness.actions).toEqual(duration <= 300 ? ['press'] : ['press', 'release']);
    expect(harness.modes).toEqual([duration <= 300]);
    expect(controller.hasIntent()).toBe(duration <= 300);
    await controller.dispose();
  });
}

test('pending startup retains a tap and only a current failed startup resets it', async () => {
  for (const success of [true, false]) {
    const starting = deferred();
    const harness = createHarness({ starting });
    const controller = createHotkeyController(harness.options);
    await controller.start();
    await controller.configure('Fn');
    harness.fnListener({ state: 'pressed' });
    harness.fnListener({ state: 'released' });
    harness.clock.advance(80);
    expect(harness.actions).toEqual(['press']);
    starting.resolve(success);
    await waitUntil(() => success || harness.actions.includes('cancel'));
    expect(harness.actions).toEqual(success ? ['press'] : ['press', 'cancel']);
    expect(harness.modes).toEqual(success ? [true] : [true, false]);
    if (!success) {
      harness.fnListener({ state: 'pressed' });
      expect(harness.actions).toEqual(['press', 'cancel', 'press']);
    }
    await controller.dispose();
  }
});

for (const outcome of ['success', 'failure', 'rejection']) {
  test(`reset ignores an old startup ${outcome} during a newer capture`, async () => {
    const starting = deferred();
    const harness = createHarness({ starting });
    const controller = createHotkeyController(harness.options);
    await controller.start();
    await controller.configure('Fn');
    harness.fnListener({ state: 'pressed' });
    controller.reset();
    harness.fnListener({ state: 'pressed' });
    if (outcome === 'rejection') starting.reject(new Error('old start failed'));
    else starting.resolve(outcome === 'success');
    for (let i = 0; i < 5; i += 1) await Promise.resolve();
    expect(harness.actions).toEqual(['press', 'press']);
    expect(harness.modes).toEqual([false]);
    await controller.dispose();
  });
}

test('a stale release timer cannot settle a newer pending release even after its deadline', async () => {
  const harness = createHarness();
  const controller = createHotkeyController(harness.options);
  await controller.start();
  await controller.configure('Fn');
  harness.fnListener({ state: 'pressed' });
  harness.clock.advance(400);
  harness.fnListener({ state: 'released' });
  const staleTimer = harness.clock.callbacks.at(-1);
  controller.reset();
  harness.fnListener({ state: 'pressed' });
  harness.clock.advance(301);
  harness.fnListener({ state: 'released' });
  const currentTimer = harness.clock.callbacks.at(-1);
  harness.clock.elapse(80);
  staleTimer();
  expect(harness.actions).toEqual(['press', 'press']);
  currentTimer();
  expect(harness.actions).toEqual(['press', 'press', 'release']);
  await controller.dispose();
});

test('registration falls back and disposal removes every source', async () => {
  const harness = createHarness({ failShortcut: 'Primary' });
  const controller = createHotkeyController(harness.options);
  await controller.start();
  expect(await controller.configure('Primary', 'Fallback')).toBe('Fallback');
  harness.shortcutListener({ state: 'Pressed' });
  await controller.dispose();

  expect(harness.actions).toEqual(['press', 'cancel']);
  expect(harness.unlistened).toBe(1);
  expect(harness.unregistered).toContain('Fallback');
  expect(harness.fnEnabled.at(-1)).toBe(false);
});

test('late shortcut registration is undone after disposal', async () => {
  const registration = deferred();
  const harness = createHarness({ registration });
  const controller = createHotkeyController(harness.options);
  const configuring = controller.configure('SlowShortcut');
  await waitUntil(() => harness.registrationStarted);
  const disposing = controller.dispose();
  registration.resolve();
  await Promise.all([configuring, disposing]);

  expect(harness.unregistered).toContain('SlowShortcut');
  expect(controller.getActiveShortcut()).toBeUndefined();
});

test('stale configuration cannot disable a newer native shortcut', async () => {
  const unregistration = deferred();
  const harness = createHarness();
  const controller = createHotkeyController(harness.options);
  await controller.configure('Previous');
  harness.unregistration = unregistration;

  const stale = controller.configure('NextGlobal');
  await Promise.resolve();
  const latest = controller.configure('Fn');
  unregistration.resolve();
  await Promise.all([stale, latest]);

  expect(controller.getActiveShortcut()).toBe('Fn');
  expect(harness.fnEnabled.at(-1)).toBe(true);
});

test('dispose reconciles a native enable that resolves late', async () => {
  const nativeEnable = deferred();
  const harness = createHarness({ nativeEnable });
  const controller = createHotkeyController(harness.options);
  const configuring = controller.configure('Fn');
  await Promise.resolve();
  const disposing = controller.dispose();
  nativeEnable.resolve();
  await Promise.all([configuring, disposing]);

  expect(controller.getActiveShortcut()).toBeUndefined();
  expect(harness.fnEnabled.at(-1)).toBe(false);
});

test('changing shortcut while held cancels that capture and accepts the next press', async () => {
  const harness = createHarness();
  const controller = createHotkeyController(harness.options);
  await controller.start();
  await controller.configure('Global');
  harness.shortcutListener({ state: 'Pressed' });

  await controller.configure('Fn');
  harness.fnListener({ state: 'pressed' });
  harness.clock.advance(301);
  harness.fnListener({ state: 'released' });
  harness.clock.advance(80);

  expect(harness.actions).toEqual(['press', 'cancel', 'press', 'release']);
});

test('reapplying the same shortcut preserves the held capture', async () => {
  const harness = createHarness();
  const controller = createHotkeyController(harness.options);
  await controller.start();
  await controller.configure('Fn', 'Fallback');
  harness.fnListener({ state: 'pressed' });

  await controller.configure('Fn', 'Fallback');

  expect(harness.actions).toEqual(['press']);
  harness.clock.advance(301);
  harness.fnListener({ state: 'released' });
  harness.clock.advance(80);
  expect(harness.actions).toEqual(['press', 'release']);
});

test('hands-free survives identical configuration but a changed shortcut cancels it', async () => {
  const harness = createHarness();
  const controller = createHotkeyController(harness.options);
  await controller.start();
  await controller.configure('Fn', 'Fallback');
  harness.fnListener({ state: 'pressed' });
  harness.fnListener({ state: 'released' });
  harness.clock.advance(80);
  await controller.configure('Fn', 'Fallback');
  expect(harness.actions).toEqual(['press']);
  expect(harness.modes).toEqual([true]);
  await controller.configure('Global', 'Fallback');
  expect(harness.actions).toEqual(['press', 'cancel']);
  expect(harness.modes).toEqual([true, false]);
  harness.shortcutListener({ state: 'Pressed' });
  expect(harness.actions).toEqual(['press', 'cancel', 'press']);
  await controller.dispose();
});

test('disposal cancels hands-free and invalidates a pending finishing release', async () => {
  const harness = createHarness();
  const controller = createHotkeyController(harness.options);
  await controller.start();
  await controller.configure('Fn');
  harness.fnListener({ state: 'pressed' });
  harness.fnListener({ state: 'released' });
  harness.clock.advance(80);
  harness.fnListener({ state: 'pressed' });
  harness.fnListener({ state: 'released' });
  const staleTimer = harness.clock.callbacks.at(-1);
  await controller.dispose();
  staleTimer();
  harness.clock.advance(1000);
  harness.fnListener({ state: 'pressed' });
  expect(harness.actions).toEqual(['press', 'cancel']);
  expect(harness.modes).toEqual([true, false]);
});

// Keep: health failure swaps sources and fallback presses toggle independently of hold time.
test('Windows hook failure registers fallback and recovery removes it before native capture', async () => {
  const h = createHarness({ nativeAvailable: false });
  const controller = createHotkeyController(h.options);
  await controller.start();
  expect(await controller.configure('Control+Windows', 'Fallback')).toBe('Fallback');
  const staleFallback = h.shortcutListener;
  staleFallback({ state: 'Pressed' });
  h.clock.advance(1000);
  staleFallback({ state: 'Released' });
  expect(h.actions).toEqual(['press']);
  staleFallback({ state: 'Pressed' });
  expect(h.actions).toEqual(['press', 'release']);
  h.options.boundary.setFnCaptureEnabled = async (enabled) => { h.fnEnabled.push(enabled); return true; };
  h.fnListener({ state: 'available' });
  await controller.configurationTail;
  expect(h.unregistered).toEqual(['Fallback']);
  expect(controller.getActiveShortcut()).toBe('Control+Windows');
  staleFallback({ state: 'Pressed' });
  expect(h.actions).toEqual(['press', 'release']);
  h.fnListener({ state: 'pressed' });
  expect(h.actions).toEqual(['press', 'release', 'press']);
  h.fnListener({ state: 'unavailable' });
  h.options.boundary.setFnCaptureEnabled = async (enabled) => { h.fnEnabled.push(enabled); return false; };
  await controller.configurationTail;
  expect(controller.getActiveShortcut()).toBe('Fallback');
  const afterFailure = [...h.actions];
  staleFallback({ state: 'Released' });
  staleFallback({ state: 'Pressed' });
  expect(h.actions).toEqual(afterFailure);
  await controller.dispose();
});

// Keep: a lifecycle event during registration must reconcile the latest requested settings.
test('recovery during initial fallback registration switches to native and respects a newer user shortcut', async () => {
  for (const changeSettings of [false, true]) {
    const registration = deferred();
    const h = createHarness({ nativeAvailable: false, registration });
    const controller = createHotkeyController(h.options);
    await controller.start();
    const configuring = controller.configure('Control+Windows', 'Fallback');
    while (!h.registrationStarted) await Promise.resolve();
    h.options.boundary.setFnCaptureEnabled = async () => true;
    if (changeSettings) void controller.configure('UserShortcut');
    h.fnListener({ state: 'available' });
    registration.resolve();
    await configuring;
    await controller.configurationTail;
    expect(h.unregistered).toContain('Fallback');
    expect(controller.getActiveShortcut()).toBe(changeSettings ? 'UserShortcut' : 'Control+Windows');
    await controller.dispose();
  }
});

// Keep: failed cleanup cannot enable native while a fallback registration remains owned.
test('recovery waits for fallback removal, including an in-flight registration cleanup failure', async () => {
  const registration = deferred();
  const h = createHarness({ nativeAvailable: false, registration });
  const controller = createHotkeyController(h.options);
  await controller.start();
  const configuring = controller.configure('Control+Windows', 'Fallback');
  while (!h.registrationStarted) await Promise.resolve();
  h.options.boundary.setFnCaptureEnabled = async (enabled) => { h.fnEnabled.push(enabled); return true; };
  h.options.boundary.unregister = async () => { throw new Error('unregister failed'); };
  h.fnListener({ state: 'available' });
  registration.resolve();
  await configuring.catch(() => undefined);
  await controller.configurationTail;
  expect(h.fnEnabled).not.toContain(true);
  expect(controller.getActiveShortcut()).not.toBe('Control+Windows');
  h.options.boundary.unregister = async () => {};
  await controller.dispose();
});

// Keep: cleanup must finish even if the OS refuses to unregister a shortcut.
test('dispose tolerates unregister failure and awaits capture cancellation', async () => {
  const h = createHarness();
  const controller = createHotkeyController(h.options);
  await controller.start();
  await controller.configure('Global');
  h.shortcutListener({ state: 'Pressed' });
  h.fnEnabled.length = 0;
  const cancellation = deferred();
  h.options.onCancel = () => cancellation.promise;
  h.options.boundary.unregister = async () => { throw new Error('unregister failed'); };
  let settled = false;
  const disposing = controller.dispose().then(() => { settled = true; });
  await new Promise(resolve => setTimeout(resolve, 0));
  expect(h.fnEnabled.at(-1)).toBe(false);
  expect(settled).toBe(false);
  cancellation.resolve();
  await expect(disposing).resolves.toBeUndefined();
});

function createHarness(config = {}) {
  const harness = {
    actions: [],
    actionTimes: [],
    clock: createClock(config.timerSkewMs),
    modes: [],
    fnEnabled: [],
    fnListener: undefined,
    shortcutListener: undefined,
    registrationStarted: false,
    unregistration: undefined,
    unlistened: 0,
    unregistered: [],
    options: undefined
  };
  harness.options = {
    clock: harness.clock,
    boundary: {
      async listenFn(listener) {
        harness.fnListener = listener;
        return () => { harness.unlistened += 1; };
      },
      async register(shortcut, listener) {
        harness.shortcutListener = listener;
        harness.registrationStarted = true;
        if (shortcut === config.failShortcut) throw new Error('conflict');
        if (config.registration) await config.registration.promise;
      },
      async setFnCaptureEnabled(enabled) {
        harness.fnEnabled.push(enabled);
        if (enabled && config.nativeEnable) await config.nativeEnable.promise;
        return config.nativeAvailable;
      },
      async unregister(shortcut) {
        harness.unregistered.push(shortcut);
        if (harness.unregistration) await harness.unregistration.promise;
      }
    },
    onCancel: () => { harness.actions.push('cancel'); harness.actionTimes.push(harness.clock.now()); },
    onHandsFreeChanged: (active) => harness.modes.push(active),
    onPress: () => {
      harness.actions.push('press');
      harness.actionTimes.push(harness.clock.now());
      if (harness.actions.filter((action) => action === 'press').length === 1 && config.starting) {
        return config.starting.promise;
      }
    },
    onRelease: () => { harness.actions.push('release'); harness.actionTimes.push(harness.clock.now()); }
  };
  return harness;
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((resolveValue, rejectValue) => { resolve = resolveValue; reject = rejectValue; });
  return { promise, resolve, reject };
}

function createClock(timerSkewMs = 0) {
  let time = 0;
  const timers = new Set();
  const callbacks = [];
  return {
    callbacks,
    now: () => time,
    schedule(callback, delay) {
      const timer = { at: time + delay - timerSkewMs, callback };
      timers.add(timer);
      callbacks.push(callback);
      return () => timers.delete(timer);
    },
    elapse(duration) { time += duration; },
    advance(duration) {
      const target = time + duration;
      for (;;) {
        const next = [...timers].sort((a, b) => a.at - b.at)[0];
        if (!next || next.at > target) break;
        time = next.at;
        timers.delete(next);
        next.callback();
      }
      time = target;
    }
  };
}

async function waitUntil(predicate) {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    if (predicate()) return;
    await Promise.resolve();
  }
  throw new Error('Condition was not met');
}

// Keep: cap completion consumes held repeats until a real release on every source.
for (const source of ['Fn', 'Control+Windows', 'Global', 'Fallback']) {
  test(`${source}: cap reset suppresses held repeats and release`, async () => {
    const h = createHarness({ nativeAvailable: source === 'Fallback' ? false : undefined });
    const controller = createHotkeyController(h.options);
    await controller.start();
    await controller.configure(source === 'Fallback' ? 'Control+Windows' : source, 'Fallback');
    const edge = pressed => ['Fn', 'Control+Windows'].includes(source)
      ? h.fnListener({ state: pressed ? 'pressed' : 'released' })
      : h.shortcutListener({ state: pressed ? 'Pressed' : 'Released' });
    edge(true);
    h.clock.advance(600_000);
    controller.reset(true);
    edge(true); edge(true);
    edge(false);
    h.clock.advance(1000);
    expect(h.actions).toEqual(['press']);
    edge(true);
    expect(h.actions).toEqual(['press', 'press']);
    await controller.dispose();
  });
}
