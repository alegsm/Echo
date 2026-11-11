import { afterEach, expect, test } from 'bun:test';
import { clientHarness } from './helpers/mount-svelte';
import { defaultSettings } from '../src/lib/core/settings';
import { attachTauriFileLogging, detachTauriFileLogging } from '../src/lib/runtime/tauri-log';
import { setNativeDebugLoggingEnabled } from '../src/lib/runtime/debug-logging';

const probes: Array<() => void> = [];
const logMessages: unknown[] = [];
let restore: (() => void) | undefined;

function nativeWindow(surface = 'dashboard') {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'window');
  Object.defineProperty(globalThis, 'window', { configurable: true, value: {
    location: { search: `?surface=${surface}` },
    __TAURI_INTERNALS__: {
      invoke(command: string, args?: { request?: unknown }) {
        if (command === 'probe_diagnostic_log') return new Promise<void>(resolve => probes.push(resolve));
        if (command === 'record_diagnostic_event') { logMessages.push(args?.request); return Promise.resolve(true); }
        throw new Error(`Unexpected native command: ${command}`);
      }
    }
  } });
  restore = () => {
    if (descriptor) Object.defineProperty(globalThis, 'window', descriptor);
    else Reflect.deleteProperty(globalThis, 'window');
  };
}
afterEach(async () => {
  setNativeDebugLoggingEnabled(undefined);
  await detachTauriFileLogging();
  while (probes.length) probes.shift()?.();
  logMessages.length = 0;
  restore?.(); restore = undefined;
});

// Keep: the production logger uses native settings only, including before startup hydration.
test.each(['dashboard', 'overlay'])('%s logger stays off without reading legacy storage', async surface => {
  nativeWindow(surface);
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  let reads = 0;
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, get() {
    reads++;
    throw new Error('Legacy storage unavailable');
  } });
  try {
    setNativeDebugLoggingEnabled(undefined);
    expect(await attachTauriFileLogging()).toBe(false);
    expect(probes).toEqual([]);
    expect(logMessages).toEqual([]);
    expect(reads).toBe(0);
  } finally {
    if (descriptor) Object.defineProperty(globalThis, 'localStorage', descriptor);
    else Reflect.deleteProperty(globalThis, 'localStorage');
  }
});

// Keep: the root page mounts each real surface once without consulting legacy storage.
test.each(['dashboard', 'overlay'])('%s root mount never reads legacy storage', async surface => {
  const page = await clientHarness(new URL('../src/routes/+page.svelte', import.meta.url));
  try {
    page.window.location.href = `http://localhost/?surface=${surface}`;
    let reads = 0, callbackId = 0;
    const calls: Array<[string, Record<string, unknown> | undefined]> = [];
    const unexpectedCommands: string[] = [];
    const legacyStorage = { configurable: true, get() { reads++; throw new Error('Legacy storage unavailable'); } };
    page.install('localStorage', undefined);
    Object.defineProperty(globalThis, 'localStorage', legacyStorage);
    Object.defineProperty(page.window, 'localStorage', legacyStorage);
    (page.window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    (page.window as any).__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: surface }, currentWebview: { label: surface } },
      transformCallback() { return ++callbackId; },
      async invoke(command: string, args?: Record<string, unknown>) {
        calls.push([command, args]);
        switch (command) {
          case 'plugin:event|listen': return ++callbackId;
          case 'get_persisted_settings': return { revision: 1, settings: defaultSettings };
          case 'get_desktop_runtime_snapshot': return { sequence: 1, snapshot: {
            mode: 'desktop', state: 'idle', activeProviderId: defaultSettings.providerId, activeShortcut: 'Fn'
          } };
          case 'list_persisted_history': return { cleanupStatus: 'complete', records: [], revision: 1, totalCount: 0 };
          case 'provider_api_key_presence': return { [defaultSettings.providerId]: true };
          case 'get_desktop_permissions': return { platform: 'windows', microphone: { granted: true, status: 'granted' } };
          case 'set_fn_hotkey_capture_enabled': return true;
          case 'plugin:window|is_maximized': return false;
          case 'plugin:event|unlisten':
          case 'plugin:window|set_background_color':
          case 'plugin:webview|set_webview_auto_resize':
          case 'plugin:window|set_size':
          case 'position_overlay':
          case 'plugin:window|show': return;
          default: unexpectedCommands.push(command); throw new Error(`Unexpected native command: ${command}`);
        }
      }
    };
    page.mount();
    // Drain the dynamic surface import and native startup without wall-clock waits.
    for (let i = 0; i < 100; i++) { await Promise.resolve(); page.flush(); }
    const selector = surface === 'dashboard' ? '.dashboard-shell' : 'main[data-mode="orb"]';
    expect(page.target.querySelectorAll(selector).length).toBe(1);
    expect(calls.filter(([command]) => command === 'get_persisted_settings')).toHaveLength(1);
    expect(calls.filter(([command, args]) => command === 'plugin:event|listen' && args?.event === 'echo-desktop-state')).toHaveLength(1);
    expect(calls.some(([command]) => command === 'probe_diagnostic_log' || command === 'record_diagnostic_event')).toBe(false);
    expect(reads).toBe(0);
    expect(unexpectedCommands).toEqual([]);
    await page.unmount();
    for (let i = 0; i < 20; i++) await Promise.resolve();
    expect(reads).toBe(0);
  } finally { await page.cleanup(); }
});

test('file logging probe cannot re-enable diagnostics after a delayed off transition', async () => {
  nativeWindow();
  setNativeDebugLoggingEnabled(true);
  const delayedEnable = attachTauriFileLogging();
  expect(probes).toHaveLength(1);
  setNativeDebugLoggingEnabled(false);
  await detachTauriFileLogging();
  probes.shift()?.();
  expect(await delayedEnable).toBe(false);

  setNativeDebugLoggingEnabled(true);
  const firstEnable = attachTauriFileLogging();
  setNativeDebugLoggingEnabled(false);
  const rapidDisable = detachTauriFileLogging();
  setNativeDebugLoggingEnabled(true);
  const finalEnable = attachTauriFileLogging();
  expect(probes).toHaveLength(2);
  probes.pop()?.(); probes.pop()?.();
  await Promise.all([firstEnable, rapidDisable, finalEnable]);
  expect(await finalEnable).toBe(true);
  expect(logMessages).toEqual([{ area: 'diagnostics', event: 'enabled' }]);
});
