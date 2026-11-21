import { expect, test } from 'bun:test';
import { clientHarness } from './helpers/mount-svelte';
import { sourcePath } from './helpers/render-svelte';
import { createUpdateController, type UpdateView } from '../src/lib/ui/dashboard/nav/update-controller';
import type { UpdateClient, UpdateEnvelope } from '../src/lib/runtime/desktop/update-client';

const appVersion: string = (await Bun.file(new URL('../package.json', import.meta.url)).json()).version;

// The intact navigation inside a minimal dashboard context. `motion` mirrors DashboardApp's motionAllowed.
async function mountNav(options: { tauri: boolean }) {
  const harness = await clientHarness(`<script>
    import DashboardNav from '${sourcePath('src/lib/ui/dashboard/nav/DashboardNav.svelte')}';
    import { provideDashboardContext } from '${sourcePath('src/lib/ui/dashboard/context.ts')}';
    import { defaultSettings } from '${sourcePath('src/lib/core/settings.ts')}';
    provideDashboardContext({ settings: () => defaultSettings, permissions: () => undefined, permissionCheckState: () => 'checking' });
    let motion = $state(true);
    export const view = { motion: (next) => { motion = next; } };
  </script><DashboardNav motionAllowed={motion} activeTab="history" onselect={() => {}} />`);
  const commands: [string, unknown][] = [];
  const callbacks = new Map<number, (event: { payload: UpdateEnvelope }) => void>();
  let callbackId = 0, statusListener: number | undefined;
  let installResponse: UpdateEnvelope = { sequence: 0, status: { state: 'none' } };
  if (options.tauri) harness.install('isTauri', true);
  (harness.window as any).__TAURI_INTERNALS__ = {
    transformCallback(fn: (event: { payload: UpdateEnvelope }) => void) { callbacks.set(++callbackId, fn); return callbackId; },
    unregisterCallback(id: number) { callbacks.delete(id); },
    async invoke(command: string, args: any) {
      commands.push([command, command === 'plugin:event|listen' ? args.event : args]);
      if (command === 'plugin:event|listen') { if (args.event === 'echo-update-status') statusListener = args.handler; return args.handler; }
      if (command === 'plugin:event|unlisten') return;
      if (command === 'get_update_status') return { sequence: 1, status: { state: 'none' } };
      if (command === 'install_update') return installResponse;
      throw new Error(`Unexpected native command: ${command}`);
    }
  };
  (harness.window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  const { view } = harness.mount() as { view: { motion(next: boolean): void } };
  const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); harness.flush(); };
  await settle();
  const button = () => harness.target.querySelector('.dashboard-version button') as unknown as HTMLButtonElement | null;
  return {
    harness, commands, view, settle, button,
    version: () => harness.target.querySelector('.dashboard-version')!,
    announcement: () => harness.target.querySelector('.dashboard-version [role="status"]')?.textContent ?? null,
    emit(envelope: UpdateEnvelope) { callbacks.get(statusListener!)!({ payload: envelope }); harness.flush(); },
    respondToInstall(envelope: UpdateEnvelope) { installResponse = envelope; },
    async click() { button()!.click(); harness.settleEvents(); await settle(); },
    installs: () => commands.filter(([command]) => command === 'install_update')
  };
}

// Keep: the sidebar loop from offer to retry through the real Tauri event and invoke APIs.
test('sidebar update button walks available, downloading, waiting, installing and failed, and retries', async () => {
  const nav = await mountNav({ tauri: true });
  try {
    // Subscribe before reading, so a change between the two is never lost.
    expect(nav.commands.slice(0, 2)).toEqual([['plugin:event|listen', 'echo-update-status'], ['get_update_status', {}]]);
    expect(nav.button()).toBeNull();
    expect(nav.version().textContent!.trim()).toEndWith(`v${appVersion}`);
    expect(nav.version().classList.contains('has-update')).toBe(false);
    expect(nav.announcement()).toBe('');

    nav.emit({ sequence: 2, status: { state: 'available', version: '0.5.1' } });
    expect(nav.button()!.getAttribute('type')).toBe('button');
    expect(nav.button()!.getAttribute('aria-label')).toBe('Update Echo to v0.5.1');
    expect(nav.button()!.getAttribute('title')).toBe('Update Echo to v0.5.1');
    expect(nav.button()!.getAttribute('aria-disabled')).toBeNull();
    expect(nav.button()!.querySelector('path')!.getAttribute('d')).toBe('M12 4v11M7.5 10.5 12 15l4.5-4.5M5 19h14');
    expect(nav.version().classList.contains('has-update')).toBe(true);
    expect(nav.announcement()).toBe('');

    nav.respondToInstall({ sequence: 3, status: { state: 'downloading', version: '0.5.1', downloadedBytes: 0, totalBytes: null } });
    await nav.click();
    expect(nav.installs()).toEqual([['install_update', {}]]);
    expect(nav.button()!.getAttribute('aria-label')).toBe('Downloading Echo v0.5.1');
    expect(nav.button()!.getAttribute('aria-disabled')).toBe('true');
    expect(nav.button()!.querySelector('.ring-arc')!.getAttribute('stroke-dasharray')).toBe('25 100');
    expect(nav.announcement()).toBe('Downloading Echo v0.5.1');
    nav.view.motion(false); nav.harness.flush();
    expect(nav.button()!.querySelector('.ring-arc')!.classList.contains('spinning')).toBe(false);
    nav.view.motion(true); nav.harness.flush();
    expect(nav.button()!.querySelector('.ring-arc')!.classList.contains('spinning')).toBe(true);

    nav.emit({ sequence: 4, status: { state: 'downloading', version: '0.5.1', downloadedBytes: 512, totalBytes: 1024 } });
    expect(nav.button()!.getAttribute('aria-label')).toBe('Downloading Echo v0.5.1, 50%');
    expect(nav.button()!.querySelector('.ring-arc')!.getAttribute('stroke-dasharray')).toBe('50 100');
    expect(nav.button()!.querySelector('.ring-arc')!.classList.contains('spinning')).toBe(false);
    expect(nav.announcement()).toBe('Downloading Echo v0.5.1');

    // A busy button sends nothing, and stale or repeated sequences change nothing.
    await nav.click();
    expect(nav.installs()).toHaveLength(1);
    nav.emit({ sequence: 4, status: { state: 'available', version: '0.5.1' } });
    nav.emit({ sequence: 2, status: { state: 'failed', version: '0.5.1' } });
    expect(nav.button()!.getAttribute('aria-label')).toBe('Downloading Echo v0.5.1, 50%');

    nav.emit({ sequence: 5, status: { state: 'waiting_for_idle', version: '0.5.1' } });
    expect(nav.button()!.getAttribute('aria-label')).toBe('Echo v0.5.1 installs when this dictation ends');
    expect(nav.button()!.getAttribute('aria-disabled')).toBe('true');
    expect(nav.announcement()).toBe('Echo v0.5.1 installs when this dictation ends');

    nav.emit({ sequence: 6, status: { state: 'installing', version: '0.5.1' } });
    expect(nav.button()!.getAttribute('aria-label')).toBe('Installing Echo v0.5.1');
    expect(nav.button()!.getAttribute('aria-disabled')).toBe('true');
    expect(nav.button()!.querySelector('.ring-arc')!.getAttribute('stroke-dasharray')).toBe('100 100');

    nav.emit({ sequence: 7, status: { state: 'failed', version: '0.5.1' } });
    expect(nav.button()!.getAttribute('aria-label')).toBe('Update failed. Try again');
    expect(nav.button()!.getAttribute('title')).toBe('Update failed. Try again');
    expect(nav.button()!.getAttribute('aria-disabled')).toBeNull();
    expect(nav.button()!.classList.contains('failed')).toBe(true);
    expect(nav.announcement()).toBe('Update failed. Try again');

    nav.respondToInstall({ sequence: 8, status: { state: 'downloading', version: '0.5.1', downloadedBytes: 0, totalBytes: 2048 } });
    await nav.click();
    expect(nav.installs()).toHaveLength(2);
    expect(nav.button()!.getAttribute('aria-label')).toBe('Downloading Echo v0.5.1, 0%');

    nav.emit({ sequence: 9, status: { state: 'none' } });
    expect(nav.button()).toBeNull();
    expect(nav.version().classList.contains('has-update')).toBe(false);

    await nav.harness.unmount();
    expect(nav.commands.at(-1)).toEqual(['plugin:event|unlisten', { event: 'echo-update-status', eventId: 1 }]);
  } finally { await nav.harness.cleanup(); }
});

// Keep: outside Tauri the navigation makes no update request, like BackgroundPanel's login-item read.
test('sidebar outside Tauri makes no update request and shows only the version', async () => {
  const nav = await mountNav({ tauri: false });
  try {
    expect(nav.commands).toEqual([]);
    expect(nav.button()).toBeNull();
    expect(nav.version().textContent!.trim()).toEndWith(`v${appVersion}`);
  } finally { await nav.harness.cleanup(); }
});

// Keep: a status read that resolves after a newer event cannot roll the view back.
test('controller drops a status read older than an event it already applied', async () => {
  let emit!: (envelope: UpdateEnvelope) => void, resolveRead!: (envelope: UpdateEnvelope) => void;
  const calls: string[] = [];
  const client: UpdateClient = {
    async listen(listener) { calls.push('listen'); emit = listener; return () => { calls.push('unlisten'); }; },
    status() { calls.push('status'); return new Promise((resolve) => { resolveRead = resolve; }); },
    async install() { throw new Error('not used'); }
  };
  const state: { view: UpdateView } = { view: { kind: 'hidden' } };
  const stop = createUpdateController(client, state).mount();
  for (let i = 0; i < 5; i++) await Promise.resolve();
  expect(calls).toEqual(['listen', 'status']);
  emit({ sequence: 5, status: { state: 'available', version: '0.5.1' } });
  resolveRead({ sequence: 4, status: { state: 'none' } });
  for (let i = 0; i < 5; i++) await Promise.resolve();
  expect(state.view).toEqual({ kind: 'available', version: '0.5.1' });
  stop();
  expect(calls).toEqual(['listen', 'status', 'unlisten']);
});
