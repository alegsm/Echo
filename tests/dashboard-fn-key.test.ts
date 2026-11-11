import { createFnKeyPreferenceController, type FnKeyState } from '../src/lib/ui/dashboard/panels/fn-key-preference-controller';
import { beforeAll, expect, test } from 'bun:test';
import { compileRenderer } from './helpers/render-svelte';

let render: (state: unknown) => string;
beforeAll(async () => {
  const fixture = await compileRenderer(new URL('./helpers/DashboardFixture.svelte', import.meta.url));
  render = (fnKey) => fixture({ component: 'fn-key', fnKey });
}, 60_000);

// Keep: dashboard handlers cross the real client with only the native invoke boundary simulated.
async function harness(current: number | string | null = 2, supported = true, desktop = true) {
  const { createFnKeyPreferenceClient } = await import('../src/lib/runtime/desktop/fn-key-preference-client');
  let previous: number | string | null | undefined;
  let failed = false;
  let release: (() => void) | undefined;
  const calls: Array<[string, Record<string, unknown> | undefined]> = [];
  const client = createFnKeyPreferenceClient({
    async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
      calls.push([command, args]);
      if (release) await new Promise<void>((resolve) => { release = resolve; });
      if (failed) throw new Error('private native detail');
      if (command === 'change_fn_key_preference') {
        if (args?.action === 'restore') { current = previous ?? null; previous = undefined; }
        else if (args?.action === 'system_default') { current = null; }
        else { if (previous === undefined) previous = current; current = 0; }
      }
      return { supported, current, canRestore: previous !== undefined } as T;
    }
  });
  const state: FnKeyState = { preference: undefined, pending: false, message: '' };
  let recheck: { focus(): void } | undefined;
  let activeElement: Element | null = null;
  const controller = createFnKeyPreferenceController(client, state, {
    tick: async () => {}, activeElement: () => activeElement,
    focusRecheck: () => recheck?.focus(), openSettings: async () => {}, isDesktop: () => desktop
  });
  const actions = { ...controller, setFocused: (element: Element) => { activeElement = element; }, setRecheck: (button: { focus(): void }) => { recheck = button; }, state: () => state };
  return { actions, calls, mount: () => controller.mount(), fail: (value: boolean) => { failed = value; },
    hold() { release = () => {}; }, release() { const resolve = release; release = undefined; resolve?.(); } };
}

test.each([null, 1, 2, 3])('Fn dashboard reads without writing, changes on click and restores %s', async (value) => {
  const h = await harness(value);
  await h.actions.refresh();
  expect(h.calls).toEqual([['get_fn_key_preference', undefined]]);
  expect(h.actions.state().preference).toEqual({ supported: true, current: value, canRestore: false });
  await h.actions.changePreference('use_fn');
  expect(h.calls[1]).toEqual(['change_fn_key_preference', { action: 'use_fn' }]);
  expect(h.actions.state().preference).toEqual({ supported: true, current: 0, canRestore: true });
  await h.actions.changePreference('restore');
  expect(h.calls[3]).toEqual(['change_fn_key_preference', { action: 'restore' }]);
  expect(h.actions.state().preference).toEqual({ supported: true, current: value, canRestore: false });
});

test('Fn actions reject repeat activation while pending and recover from native failures', async () => {
  const h = await harness();
  await h.actions.refresh();
  h.hold();
  const changing = h.actions.changePreference('use_fn');
  const repeated = h.actions.changePreference('restore');
  const refresh = h.actions.refresh();
  expect(h.calls).toHaveLength(2);
  h.release();
  await Promise.all([changing, repeated, refresh]);
  h.fail(true);
  await h.actions.changePreference('restore');
  expect(h.actions.state().message).toContain('could not');
  expect(h.actions.state().message).not.toContain('private');
  expect(h.actions.state().preference!.canRestore).toBe(true);
  expect(h.actions.state().pending).toBe(false);
  h.fail(false);
  await h.actions.changePreference('restore');
  expect(h.actions.state().preference!.canRestore).toBe(false);
});

test('Fn controls show named values in a Permissions row with Settings and safe actions', () => {
  expect(render({ preference: { supported: false, current: null, canRestore: false } })).not.toContain('Use Fn for Echo');
  const html = render({ preference: { supported: true, current: 2, canRestore: true } });
  for (const text of ['Use Fn for Echo', 'Restore previous setting', 'Current setting:', 'Show Emoji &amp; Symbols', 'Settings', 'Log out and back in']) expect(html).toContain(text);
  expect(html).toMatch(/class="purpose(?: |")/);
  expect(render({ preference: { supported: true, current: null, canRestore: false } })).toContain('System default');
});

test.each([99, -1, 1.5, 'unrecognized'])('Fn control keeps unrecognized value %s visible and disables Use Fn', async (current) => {
  const h = await harness(current);
  const html = render({ preference: { supported: true, current, canRestore: false } });
  expect(html).toContain('Echo will not change an unrecognized value');
  expect(html).toContain('Unrecognized value');
  expect(html).toMatch(/class="recheck[^"]*"[^>]*>Re-check Fn setting/);
  expect(html).toContain('>Settings</button>');
  expect(html).toMatch(/<button[^>]*disabled[^>]*>Use Fn for Echo/);
  await h.actions.refresh();
  await h.actions.changePreference('use_fn');
  expect(h.calls).toHaveLength(1);
});

test('Fn control offers system default for a value with no restore record', () => {
  expect(render({ preference: { supported: true, current: 0, canRestore: false } })).toContain('Use system default');
  expect(render({ preference: { supported: true, current: null, canRestore: false } })).not.toContain('Use system default');
});

// Keep: browser mounts never cross a native boundary or show an invocation error.
test('Fn control stays hidden in a plain browser', async () => {
  const h = await harness(2, true, false);
  h.mount();
  await Promise.resolve();
  await h.actions.refresh();
  expect(h.calls).toEqual([]);
  expect(h.actions.state().preference).toBeUndefined();
  expect(h.actions.state().message).toBe('');
});

// Keep: the focused action becoming disabled transfers focus to the persistent re-check button.
test('Use Fn moves focus to re-check after success', async () => {
  const h = await harness();
  await h.actions.refresh();
  let focused = false;
  h.actions.setRecheck({ focus() { focused = true; } });
  const trigger = { isConnected: true, get disabled() { return h.actions.state().preference!.current === 0; } };
  h.actions.setFocused(trigger as HTMLButtonElement);
  await h.actions.changePreference('use_fn', trigger as HTMLButtonElement);
  expect(focused).toBe(true);
});

// Keep: old installs without a backup can explicitly remove a set value through the native client.
test.each([0, 1, 2, 3])('Use system default resets %s without inventing a restore record', async (current) => {
  const h = await harness(current);
  await h.actions.refresh();
  await h.actions.changePreference('system_default');
  expect(h.calls).toEqual([
    ['get_fn_key_preference', undefined],
    ['change_fn_key_preference', { action: 'system_default' }],
    ['get_fn_key_preference', undefined]
  ]);
  expect(h.actions.state().preference).toEqual({ supported: true, current: null, canRestore: false });
});

// Keep: the read failure keeps a named row and a separate action group for Re-check.
test('Fn read failure renders Globe tap action and recovers through re-check', async () => {
  const h = await harness();
  h.fail(true); await h.actions.refresh();
  expect(h.actions.state().message).toBe('Fn key setting could not be checked. Try again.');
  const html = render(h.actions.state());
  expect(html).toMatch(/class="purpose(?: |")[^]*?Globe tap action[^]*?role="alert"/);
  expect(html).toMatch(/class="actions(?: |")[^]*?Re-check Fn setting/);
  expect(html).not.toContain('private native detail');
  h.fail(false); await h.actions.refresh();
  expect(h.actions.state().preference!.current).toBe(2);
  expect(h.actions.state().message).toBe('');
});
