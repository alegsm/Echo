import { beforeAll, expect, test } from 'bun:test';
import { fileURLToPath } from 'node:url';
import { Window } from 'happy-dom';
import { compile } from 'svelte/compiler';
import { compileRenderer } from './helpers/render-svelte';
import { desktopPlatform, createPermissionsController } from '../src/lib/ui/dashboard/panels/permissions-controller';

let render: Awaited<ReturnType<typeof compileRenderer>>;
beforeAll(async () => {
  render = await compileRenderer(new URL('./helpers/DashboardFixture.svelte', import.meta.url));
}, 60_000);

const item = (status: string) => ({ status, granted: status === 'authorized', message: '' });
function permissions(platform: 'macos' | 'windows', microphone = 'authorized') {
  return {
    platform, microphone: item(microphone),
    accessibility: item(platform === 'macos' ? 'authorized' : 'not_required'),
    input_monitoring: item(platform === 'macos' ? 'authorized' : 'not_required')
  };
}

// Keep: the native platform controls rows, counts, rail readiness and credential copy on both entry paths.
for (const platform of ['macos', 'windows'] as const) {
  for (const tab of ['settings', 'setup'] as const) {
    test(`${platform} ${tab} shows only applicable permissions and a truthful rail`, () => {
      const native = permissions(platform);
      const props = { initialState: { tab, permissions: native, permissionCheckState: 'ready',
        settings: { pushToTalkShortcut: platform === 'windows' ? 'Control+Windows' : 'Fn' } } };
      const html = render(props);
      const list = html.match(/<ul[^>]*aria-label="Desktop permissions"[^>]*>([\s\S]*?)<\/ul>/)![1];
      expect(list.match(/<li\b/g)).toHaveLength(platform === 'windows' ? 1 : 3);
      expect(list).toContain('Microphone');
      expect(list.includes('Accessibility')).toBe(platform === 'macos');
      expect(list.includes('Input Monitoring')).toBe(platform === 'macos');
      expect(html).toContain(platform === 'windows' ? '1/1 granted' : '3/3 granted');
      expect(html).toContain('Permissions granted');
      if (platform === 'windows') expect(html).not.toMatch(/Globe|Fn key|System Settings/);
      const unknown = render({ initialState: { ...props.initialState, permissions: permissions(platform, 'unknown') } });
      expect(unknown).toContain(platform === 'windows' ? '0/1 granted' : '2/3 granted');
      expect(unknown).toContain('Permissions need attention');
      expect(unknown).toContain('aria-label="Settings Microphone"');
      const denied = render({ initialState: { ...props.initialState, permissions: permissions(platform, 'denied') } });
      expect(denied).toContain(`aria-label="${platform === 'windows' ? 'Settings' : 'Grant'} Microphone"`);
    });
  }

  test(`${platform} names the credential store throughout Settings and setup`, () => {
    for (const tab of ['settings', 'setup'] as const) {
      const html = render({ initialState: { tab, permissions: permissions(platform), permissionCheckState: 'ready',
        setup: { step: 2 } } });
      const store = platform === 'windows' ? 'Credential Manager' : 'Keychain';
      expect(html).not.toContain('Where the key lives');
      expect(html).toContain(`Paste your Groq key`);
      expect(html).toContain(`${platform === 'windows' ? 'Windows Credential Manager' : 'macOS Keychain'} on this device`);
      expect(html.includes('macOS may ask you to allow Echo access')).toBe(platform === 'macos');
      expect(html).toContain(`key from ${store}?`);
      expect(html).not.toContain(platform === 'windows' ? 'Keychain' : 'Credential Manager');
    }
  });

  // Keep: Find saved key renders only for an unconfigured provider, with the macOS consent note on macOS only.
  test(`${platform} offers Find saved key only for an unconfigured provider`, () => {
    for (const tab of ['settings', 'setup'] as const) {
      for (const configured of [false, true]) {
        const html = render({ initialState: { tab, permissions: permissions(platform), permissionCheckState: 'ready',
          setup: { step: 2 }, providerKeyPresence: configured ? { 'groq-whisper-turbo': true } : {} } });
        const find = html.match(/<div class="find-saved[^"]*">([\s\S]*?)<\/div>/);
        expect(Boolean(find)).toBe(!configured);
        expect(html.includes('>Remove key</button>')).toBe(configured);
        // Remove key shares the key-action button shape; its leading icon is decorative, so the name stays exactly "Remove key".
        expect(/<button(?=[^>]*type="button")(?=[^>]*class="[^"]*\bkey-action\b[^"]*\bremove-key\b)(?![^>]*disabled)[^>]*>(?:<!--[^>]*-->)*<svg(?=[^>]*aria-hidden="true")(?=[^>]*focusable="false")[^>]*>(?:(?!<\/svg>)[\s\S])*<\/svg>Remove key<\/button>/.test(html)).toBe(configured);
        if (!find) continue;
        // The leading icon is decorative, so the accessible name stays exactly "Find saved key".
        expect(find[1]).toMatch(/<button(?=[^>]*type="button")(?![^>]*disabled)[^>]*>(?:<!--[^>]*-->)*<svg(?=[^>]*aria-hidden="true")(?=[^>]*focusable="false")[^>]*>(?:(?!<\/svg>)[\s\S])*<\/svg>Find saved key<\/button>/);
        expect(find[1].includes('macOS may ask you to allow Echo access.')).toBe(platform === 'macos');
      }
    }
  });

  test(`${platform} settings recovery reports the correct settings application`, async () => {
    const feedback: { message: string | null } = { message: null };
    const ctx = {
      permissions: () => permissions(platform), refreshPermissions: async () => {}, setPermissions() {},
      setActionError(value: string | null) { feedback.message = value; },
      runtime: () => ({ openDesktopPermissionSettings: async () => { throw new Error('unavailable'); } })
    };
    const controller = createPermissionsController(ctx as unknown as Parameters<typeof createPermissionsController>[0],
      { permissionAction: undefined, permissionRefreshState: 'idle' },
      { tick: async () => {}, activeElement: () => null, focusHeading() {}, refreshFn: async () => {} });
    await controller.openPermissionSettings('microphone');
    expect(feedback.message).toBe(`Could not open ${platform === 'windows' ? 'Settings' : 'System Settings'}. Try again.`);
  });
}

// Keep: the Windows bar sits over the reserved top band whichever stylesheet the bundler emits last.
// Happy-dom resolves the cascade; rendered geometry and native drag still need the running app.
test('Windows title bar stays out of grid flow at the top in either stylesheet order', async () => {
  const titleBar = new URL('../src/lib/ui/dashboard/TitleBar.svelte', import.meta.url);
  const component = compile(await Bun.file(titleBar).text(), { filename: fileURLToPath(titleBar), css: 'external' }).css!.code;
  const shell = await Bun.file(new URL('../src/lib/ui/dashboard/dashboard-app.css', import.meta.url)).text();
  const html = render({ initialState: { permissions: permissions('windows'), permissionCheckState: 'ready' } });
  for (const sheets of [[shell, component], [component, shell]]) {
    const window = new Window();
    try {
      window.document.head.innerHTML = sheets.map(css => `<style>${css}</style>`).join('');
      window.document.body.innerHTML = html;
      const main = window.document.querySelector('.dashboard-main')!;
      const bar = main.querySelector(':scope > .titlebar')!;
      expect(main.firstElementChild).toBe(bar);
      const style = window.getComputedStyle(bar);
      expect([style.position, style.top, style.left, style.right]).toEqual(['absolute', '0px', '0px', '0px']);
      expect(window.getComputedStyle(main).paddingTop).toBe('32px');
      expect(bar.querySelector('[data-tauri-drag-region]')).not.toBeNull();
      expect([...bar.querySelectorAll('button')].map(button => button.getAttribute('aria-label'))).toEqual(['Minimize', 'Maximize', 'Close']);
    } finally { await window.happyDOM.close(); }
  }
});

// Keep: native platform takes precedence over the browser hint; the hint works before native data arrives.
test('native platform takes precedence over conflicting browser hints', () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
  try {
    for (const platform of ['macos', 'windows'] as const) {
      const hint = platform === 'macos' ? 'Windows' : 'Macintosh';
      Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { platform: hint, userAgent: hint } });
      expect(desktopPlatform(permissions(platform))).toBe(platform);
      expect(desktopPlatform()).toBe(platform === 'macos' ? 'windows' : 'macos');
    }
  } finally {
    if (original) Object.defineProperty(globalThis, 'navigator', original);
    else Reflect.deleteProperty(globalThis, 'navigator');
  }
});
