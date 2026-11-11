import { expect, test } from 'bun:test';
import { clientHarness } from './helpers/mount-svelte';
import { sourcePath } from './helpers/render-svelte';
import { particleTarget } from '../src/lib/ui/overlay/particle-halo-geometry';

// Keep: a parent colour update between frames must not reach HaloCircles early.
test('ParticleHalo renders tick-sampled colours and coordinates through HaloCircles', async () => {
  const halo = await clientHarness(`<script>
    import Halo from '${sourcePath('src/lib/ui/overlay/ParticleHalo.svelte')}';
    let colors = $state(Array(6).fill('#112233'));
    export function recolor() { colors = Array(6).fill('#aabbcc'); }
  </script><Halo {colors} pose="halo" particleCount={60} animationSpeed={1} transitionMs={0} />`);
  try {
    const component = halo.mount() as { recolor(): void };
    const circles = () => [...halo.target.querySelectorAll('circle')];
    expect(circles()).toHaveLength(60);
    halo.frame(0);
    component.recolor(); halo.flush();
    expect(circles().map(circle => circle.getAttribute('fill'))).toEqual(Array(60).fill('#112233'));
    halo.frame(1000);
    expect(circles().map(circle => circle.getAttribute('fill'))).toEqual(Array(60).fill('#aabbcc'));
    circles().forEach((circle, i) => {
      const target = particleTarget('halo', i, 60, 1);
      expect(Number(circle.getAttribute('cx'))).toBeCloseTo(target.x, 9);
      expect(Number(circle.getAttribute('cy'))).toBeCloseTo(target.y, 9);
    });
    await halo.unmount(); expect(halo.frames.size).toBe(0);
  } finally { await halo.cleanup(); }
});

// Keep: neither initial hydration nor an unsupported refresh reveals the Fn row.
test.each([false, true])('Fn row stays hidden during the first read (supported: %s)', async supported => {
  const fn = await clientHarness(new URL('../src/lib/ui/dashboard/panels/FnKeyPreferenceControl.svelte', import.meta.url));
  try {
    let resolveRead!: (value: unknown) => void;
    let reads = 0;
    fn.install('isTauri', true);
    (fn.window as any).__TAURI_INTERNALS__ = { invoke: () => {
      reads++;
      return new Promise(resolve => { resolveRead = resolve; });
    } };
    const component = fn.mount() as { refresh(): Promise<void> };
    expect(reads).toBe(1);
    expect(fn.target.querySelectorAll('.fn-control').length).toBe(0);
    resolveRead({ supported, current: null, canRestore: false });
    for (let i = 0; i < 10; i++) await Promise.resolve(); fn.flush();
    expect(Boolean(fn.target.querySelector('.fn-control'))).toBe(supported);
    if (!supported) {
      const pending = component.refresh(); fn.flush();
      expect(reads).toBe(2);
      expect(fn.target.querySelectorAll('.fn-control').length).toBe(0);
      resolveRead({ supported: false, current: null, canRestore: false });
      await pending; fn.flush();
      expect(fn.target.querySelectorAll('.fn-control').length).toBe(0);
    }
  } finally { await fn.cleanup(); }
});

test('Fn failure-row Re-check keeps the focused button connected while reading', async () => {
  const fn = await clientHarness(new URL('../src/lib/ui/dashboard/panels/FnKeyPreferenceControl.svelte', import.meta.url));
  try {
    let resolveRead!: (value: unknown) => void;
    let reads = 0;
    fn.install('isTauri', true);
    (fn.window as any).__TAURI_INTERNALS__ = { invoke: () => Promise.reject(new Error('unavailable')) };
    fn.mount({ initialState: { message: 'Fn key setting could not be checked. Try again.' } });
    for (let i = 0; i < 10; i++) await Promise.resolve(); fn.flush();
    (fn.window as any).__TAURI_INTERNALS__.invoke = () => { reads++; return new Promise(resolve => { resolveRead = resolve; }); };
    const button = fn.target.querySelector('button')!;
    button.focus(); button.click(); fn.flush();
    expect(reads).toBe(1);
    expect(button.isConnected).toBe(true);
    expect(fn.window.document.activeElement).toBe(button);
    expect(button.getAttribute('aria-disabled')).toBe('true');
    resolveRead({ supported: false, current: null, canRestore: false });
    for (let i = 0; i < 10; i++) await Promise.resolve(); fn.flush();
    expect(fn.target.querySelectorAll('.fn-control').length).toBe(0);
  } finally { await fn.cleanup(); }
});

// Keep: every failed retry creates a new alert for assistive technology.
test('Fn failed re-check re-announces its alert on every attempt', async () => {
  const fn = await clientHarness(new URL('../src/lib/ui/dashboard/panels/FnKeyPreferenceControl.svelte', import.meta.url));
  try {
    fn.install('isTauri', true);
    (fn.window as any).__TAURI_INTERNALS__ = { invoke: () => Promise.reject(new Error('unavailable')) };
    const component = fn.mount() as { refresh(): Promise<void> };
    for (let i = 0; i < 10; i++) await Promise.resolve(); fn.flush();
    for (let attempt = 0; attempt < 3; attempt++) {
      const previous = fn.target.querySelector('[role="alert"]')!;
      expect(previous).not.toBeNull();
      await component.refresh(); fn.flush();
      const next = fn.target.querySelector('[role="alert"]')!;
      expect(next.textContent).toContain('could not be checked');
      expect(next === previous).toBe(false);
    }
  } finally { await fn.cleanup(); }
});

// Keep: repeated Settings launch failures replace the alert even when its text is unchanged.
test('Fn Settings failures re-announce their alert on every attempt', async () => {
  const fn = await clientHarness(new URL('../src/lib/ui/dashboard/panels/FnKeyPreferenceControl.svelte', import.meta.url));
  try {
    fn.install('isTauri', true);
    (fn.window as any).__TAURI_INTERNALS__ = { invoke: async (command: string) => {
      if (command === 'get_fn_key_preference') return { supported: true, current: 2, canRestore: false };
      throw new Error('unavailable');
    } };
    fn.mount();
    for (let i = 0; i < 10; i++) await Promise.resolve(); fn.flush();
    const settings = [...fn.target.querySelectorAll('button')].find(button => button.textContent === 'Settings')!;
    for (let attempt = 0; attempt < 3; attempt++) {
      const previous = fn.target.querySelector('[role="alert"]');
      settings.click();
      for (let i = 0; i < 10; i++) await Promise.resolve(); fn.flush();
      const next = fn.target.querySelector('[role="alert"]')!;
      expect(next.textContent).toBe('Could not open System Settings. Try again.');
      expect(next === previous).toBe(false);
    }
  } finally { await fn.cleanup(); }
});

// Keep: snapshot errors cross the native boundary and render a label matching their kind.
test('dashboard snapshot alerts distinguish provider errors from local failures', async () => {
  const dashboard = await clientHarness(new URL('../src/lib/ui/dashboard/DashboardApp.svelte', import.meta.url));
  try {
    const { defaultSettings } = await import('../src/lib/core/settings');
    const callbacks = new Map<number, (event: unknown) => void>();
    let stateListener!: (event: unknown) => void;
    let callbackId = 0;
    const snapshot = { mode: 'desktop', state: 'error', activeProviderId: defaultSettings.providerId, activeShortcut: 'Fn' };
    (dashboard.window as any).__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'dashboard' } },
      transformCallback(callback: (event: unknown) => void) { callbacks.set(++callbackId, callback); return callbackId; },
      unregisterCallback(id: number) { callbacks.delete(id); },
      invoke: async (command: string, args: any) => {
        if (command === 'plugin:event|listen') {
          if (args.event === 'echo-desktop-state') stateListener = callbacks.get(args.handler)!;
          return args.handler;
        }
        if (command === 'plugin:event|unlisten') return;
        if (command === 'plugin:window|is_maximized') return false;
        if (command === 'get_desktop_runtime_snapshot') return { sequence: 1, snapshot };
        if (command === 'get_persisted_settings') return { revision: 1, settings: defaultSettings };
        if (command === 'get_desktop_permissions') return undefined;
        if (command === 'provider_api_key_presence') return {};
        if (command === 'list_persisted_history') return { records: [], totalCount: 0, revision: 1, cleanupStatus: 'complete' };
        throw new Error(`Unexpected command: ${command}`);
      }
    };
    (dashboard.window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    dashboard.mount();
    for (let i = 0; i < 50; i++) await Promise.resolve(); dashboard.flush();
    let sequence = 1;
    for (const [lastError, label] of [
      ['shortcut_conflict', 'Action failed'], ['microphone_unavailable', 'Action failed'],
      ['microphone_permission_denied', 'Action failed'], ['capture_failed', 'Action failed'],
      ['empty_audio', 'Action failed'], ['clipboard_failed', 'Action failed'],
      ['insertion_failed', 'Action failed'], ['not_implemented', 'Action failed'],
      ['provider_timeout', 'Network Failure'], ['provider_failed', 'Network Failure'],
      ['missing_provider_key', 'Provider setup'], ['history_write_failed', 'Action failed']
    ]) {
      stateListener({ payload: { sequence: ++sequence, snapshot: { ...snapshot, lastError } } }); dashboard.flush();
      const alert = dashboard.target.querySelector('[role="alert"]')!;
      expect(alert.textContent).toContain(label);
      if (label === 'Action failed') expect(alert.textContent).not.toContain('Network Failure');
    }
    stateListener({ payload: { sequence: ++sequence, snapshot } }); dashboard.flush();
    expect(dashboard.target.querySelector('[role="alert"]')).toBeNull();
  } finally { await dashboard.cleanup(); }
});

// Keep: controller.dispose is the owner of the pending preview fade timer.
test('OverlayApp unmount disposes its controller, fade timer and runtime listeners', async () => {
  const overlay = await clientHarness(new URL('../src/lib/ui/overlay/OverlayApp.svelte', import.meta.url));
  try {
    const snapshot = { mode: 'desktop', state: 'recording', activeProviderId: 'groq-whisper-turbo', activeShortcut: 'Fn' };
    const listeners = new Set<(event: any) => void>();
    let disposed = false;
    overlay.mount({ createRuntime: () => ({
      subscribe(listener: (event: any) => void) { listeners.add(listener); return () => listeners.delete(listener); },
      getSnapshot: async () => snapshot,
      dispose() { disposed = true; }
    }) });
    await Promise.resolve(); overlay.flush();
    const emit = (event: unknown) => { listeners.forEach(listener => listener(event)); overlay.flush(); };
    emit({ type: 'partial_transcript_changed', projection: { dictationId: 'capture', phase: 'active', eligible: true, text: 'hello', ordinal: -1 } });
    emit({ type: 'transcript_completed', dictationId: 'capture', transcript: { id: 'result', text: 'hello' } });
    expect([...overlay.timers.values()].some(timer => timer.delay === 200)).toBe(true);
    expect(listeners.size).toBe(1);
    await overlay.unmount();
    expect(disposed).toBe(true); expect(listeners.size).toBe(0);
    expect(overlay.timers.size).toBe(0);
    // Svelte owns one shared tween frame, which drains after the final subscriber leaves.
    overlay.frame(1000); expect(overlay.frames.size).toBe(0);
    expect(overlay.listeners.size).toBe(0);
  } finally { await overlay.cleanup(); }
});

// Keep: native platform wins over a conflicting webview hint, including late hydration.
test('TitleBar follows native permissions when they arrive', async () => {
  const title = await clientHarness(`<script lang="ts">
    import TitleBar from '${sourcePath('src/lib/ui/dashboard/TitleBar.svelte')}';
    import { provideDashboardContext } from '${sourcePath('src/lib/ui/dashboard/context.ts')}';
    let permissions = $state(undefined);
    provideDashboardContext({ permissions: () => permissions });
    export function hydrate() { permissions = { platform: 'windows' }; }
  </script><TitleBar />`);
  try {
    Object.defineProperty(title.window.navigator, 'userAgent', { configurable: true, value: 'Macintosh' });
    (title.window as any).__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'dashboard' } },
      invoke: async () => false, transformCallback: () => 1, unregisterCallback() {}
    };
    (title.window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    const component = title.mount() as { hydrate(): void };
    expect(title.target.querySelector('.mac-drag-strip')).not.toBeNull();
    component.hydrate(); title.flush();
    for (let i = 0; i < 10; i++) await Promise.resolve(); title.flush();
    expect(title.target.querySelector('[aria-label="Minimize"]')).not.toBeNull();
    expect(title.target.querySelector('.mac-drag-strip')).toBeNull();
  } finally { await title.cleanup(); }
});

// Keep: the intact dashboard and desktop runtime reach the OS store only through an explicit Find saved key.
for (const outcome of ['found', 'not_found'] as const) for (const tab of ['settings', 'setup'] as const) {
  test(`Find saved key ${outcome} from ${tab} searches only on request and settles focus`, async () => {
    const dashboard = await clientHarness(new URL('../src/lib/ui/dashboard/DashboardApp.svelte', import.meta.url));
    try {
      const { defaultSettings } = await import('../src/lib/core/settings');
      const id = defaultSettings.providerId;
      const commands: [string, any][] = [];
      let revision = 1, release!: () => void;
      let settings: any = { ...defaultSettings, providerKeyStatuses: { [id]: 'invalid' }, providerKeySaved: { [id]: false } };
      const item = (status: string) => ({ status, granted: status === 'authorized', message: '' });
      (dashboard.window as any).__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'dashboard' } },
        transformCallback() { return 1; }, unregisterCallback() {},
        invoke: async (command: string, args: any) => {
          if (command.startsWith('plugin:')) return command === 'plugin:event|listen' ? args.handler : false;
          commands.push([command, args]);
          if (command === 'get_desktop_runtime_snapshot') return { sequence: 1, snapshot: { mode: 'desktop', state: 'idle', activeProviderId: id, activeShortcut: 'Control+Windows' } };
          if (command === 'get_persisted_settings') return { revision, settings };
          if (command === 'get_desktop_permissions') return { platform: 'windows', microphone: item('authorized'), accessibility: item('not_required'), input_monitoring: item('not_required') };
          if (command === 'provider_api_key_presence') return { [id]: settings.providerKeySaved[id] === true };
          if (command === 'list_persisted_history') return { records: [], totalCount: 0, revision: 1, cleanupStatus: 'complete' };
          if (command === 'find_provider_api_key') {
            await new Promise<void>((resolve) => { release = resolve; });
            if (outcome === 'found') { settings = { ...settings, providerKeySaved: { [id]: true } }; revision++; }
            return { outcome };
          }
          if (command === 'update_persisted_settings') {
            const previous = { revision, settings };
            settings = { ...settings, providerKeyStatuses: { ...settings.providerKeyStatuses, ...args.patch.providerKeyStatuses } };
            return { previous, current: { revision: ++revision, settings }, changedFields: Object.keys(args.patch), cleanupStatus: 'complete' };
          }
          return undefined;
        }
      };
      (dashboard.window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
      const settle = async () => { for (let i = 0; i < 50; i++) await Promise.resolve(); dashboard.flush(); };
      const button = (text: string) => [...dashboard.target.querySelectorAll('button')].find((b) => b.textContent?.trim() === text);
      dashboard.mount({ initialState: { tab, setup: { step: 2 } } });
      await settle();
      const names = () => commands.map(([name]) => name);
      expect(names()).not.toContain('find_provider_api_key');
      const find = button('Find saved key')!;
      expect(find.getAttribute('aria-busy')).toBe('false');
      find.focus(); find.click(); dashboard.flush();
      await settle();
      expect(commands.filter(([name]) => name === 'find_provider_api_key')).toEqual([['find_provider_api_key', { providerId: id }]]);
      expect(find.getAttribute('aria-busy')).toBe('true');
      expect(find.disabled).toBe(true);
      expect(find.querySelector('.check-light')).not.toBeNull();
      expect(button('Check key')!.disabled).toBe(true);
      expect(dashboard.target.querySelector('.check-result')!.textContent).toBe('Looking in Credential Manager');
      expect(dashboard.target.querySelector('.result-dot.loading')).not.toBeNull();
      const searched = commands.length;
      release(); await settle();
      const after = names().slice(searched);
      const status = dashboard.target.querySelector('.check-result')!;
      if (outcome === 'found') {
        // The runtime reloads committed settings before the status reset; presence refreshes after it.
        expect(after[0]).toBe('get_persisted_settings');
        const update = after.indexOf('update_persisted_settings');
        expect(update).toBeGreaterThan(0);
        expect(after.lastIndexOf('provider_api_key_presence')).toBeGreaterThan(update);
        expect(commands[searched + update][1].patch).toEqual({ providerKeyStatuses: { [id]: 'unchecked' } });
        expect(button('Find saved key')).toBeUndefined();
        expect(button('Remove key')).toBeDefined();
        expect(dashboard.window.document.activeElement).toBe(button('Check key')!);
        expect(status.textContent).toBe('Found your saved Groq key. Select Check key to test it.');
        expect(status.querySelector('.result-dot.success')).not.toBeNull();
        expect(dashboard.target.querySelector('[aria-label="Credential needs attention"]')).toBeNull();
      } else {
        expect(after).toEqual([]);
        expect(find.isConnected).toBe(true);
        expect(find.disabled).toBe(false);
        expect(find.getAttribute('aria-busy')).toBe('false');
        expect(find.querySelector('.check-light')).toBeNull();
        expect(dashboard.window.document.activeElement).toBe(find);
        expect(status.textContent).toBe('No saved Groq key in Credential Manager. Paste your key and select Check key.');
        expect(status.querySelector('.result-dot')).toBeNull();
      }
      expect(names()).not.toContain('set_provider_api_key');
      expect(names()).not.toContain('probe_provider');
    } finally { await dashboard.cleanup(); }
  });
}
