import { createDashboardController, initialDashboardState } from '../src/lib/ui/dashboard/dashboard-controller';
import { createKeybindsController } from '../src/lib/ui/dashboard/panels/keybinds-controller';
import { createTranscriptionController } from '../src/lib/ui/dashboard/panels/transcription-controller';
import { createPermissionsController } from '../src/lib/ui/dashboard/panels/permissions-controller';
import { createDebugLogController } from '../src/lib/ui/dashboard/panels/debug-log-controller';
import { beforeAll, expect, test } from 'bun:test';
import { fileURLToPath } from 'node:url';
import { compile } from 'svelte/compiler';
import { compileRenderer, sourcePath } from './helpers/render-svelte';
import { clientHarness } from './helpers/mount-svelte';
import { defaultSettings, providerOptions } from '../src/lib/core/settings';
import type { EchoSettings } from '../src/lib/core/settings';
import type { EchoRuntime } from '../src/lib/core/types';
import type { TranscriptionState } from '../src/lib/ui/dashboard/panels/transcription-controller';
import type { DebugLogState } from '../src/lib/ui/dashboard/panels/debug-log-controller';
import type { DesktopPermissions } from '../src/lib/core/types';
import { createNativeStorageClient } from '../src/lib/runtime/desktop/native-storage-client';

// Markup and handler proofs only. Browser interaction and frost use the host harness.
let render: (settings?: Partial<EchoSettings>, presence?: Record<string, boolean>, permissions?: DesktopPermissions, state?: 'checking' | 'ready' | 'unknown') => string;
const windowsPermissions = {
  platform: 'windows',
  microphone: { granted: true, status: 'authorized', message: 'Echo can capture audio.' },
  accessibility: { granted: false, status: 'not_required', message: 'Not required on this platform.' },
  input_monitoring: { granted: false, status: 'not_required', message: 'Not required on this platform.' }
} satisfies DesktopPermissions;
let renderKeybinds: (available?: boolean | null) => string;
beforeAll(async () => {
  const fixture = await compileRenderer(new URL('./helpers/DashboardFixture.svelte', import.meta.url));
  renderKeybinds = (available) => fixture({ component: 'keybinds',
    initialState: { settings: { pushToTalkShortcut: 'Control+Windows', fallbackShortcut: 'CommandOrControl+Shift+Space' },
      permissions: windowsPermissions },
    snapshot: { nativeModifierAvailable: available }
  });
  render = (settings = {}, presence = {}, permissions, state = permissions ? 'ready' : 'checking') => fixture({
    initialState: { tab: 'settings', settings: { restoreClipboard: false, ...settings },
      providerKeyPresence: presence, permissions, permissionCheckState: state }
  });
}, 60_000);

// Keep: compilation rejects broken imports and fixture syntax through the same public helper.
test('render compilation rejects missing imports and invalid fixtures', async () => {
  for (const compile of [
    () => compileRenderer('<script>import Missing from "./missing-fixture.svelte";</script><Missing />'),
    () => compileRenderer('<div'),
    () => compileRenderer(new URL('./helpers/missing-fixture.svelte', import.meta.url))
  ]) {
    const compilation = compile();
    await expect(compilation).rejects.toBeInstanceOf(AggregateError);
    await expect(compilation).rejects.toThrow('Test component compilation failed');
  }
});

test('Settings composes the available sections in order without section navigation', () => {
  const html = render();
  expect(html).toMatch(/<h1[^>]*>Settings<\/h1>/);
  const headings = [...html.matchAll(/<h2[^>]*>([^<]+)<\/h2>/g)].map((match) => match[1]);
  expect(headings).toEqual(['Shortcut', 'Paste', 'Provider', 'Transcription', 'Permissions', 'Background', 'Debug log']);
  expect(html).not.toContain('Settings sections');
  for (const control of ['Keychain', 'Check key', 'Desktop permissions', 'Detailed logging']) expect(html).toContain(control);
  expect(html).toMatch(/class="[^"]*dashboard-brand[^\"]*" aria-hidden="true"[^>]*>[\s\S]*class="[^"]*brand-wordmark[^\"]*"[^>]*>Echo<\/span>/);
});

test('Shortcut shows the current key and Change; Paste exposes both named switches and shows both saved values', () => {
  const html = render();
  expect(html).toMatch(/<kbd[^>]*>Fn<\/kbd>/);
  expect(html).toMatch(/<button[^>]*>Change<\/button>/);
  expect(html).toContain('hands-free');
  expect(html).toMatch(/Hold [^<]*release to transcribe\./);
  expect(html).toMatch(/Tap once [^<]*tap again to transcribe\./);
  const instructions = html.match(/>\s*(Hold [^<]*Tap once [^<]*)</)?.[1];
  expect(instructions).toBeDefined();
  expect(instructions!).not.toMatch(/\d|\bms\b/);
  for (const [label, checked] of [['Paste after transcription', true], ['Restore clipboard after paste', false]]) {
    expect(html).toMatch(new RegExp(`<button[^>]*role="switch"[^>]*aria-checked="${checked}"[^>]*aria-label="${label}"`));
  }
});

test('Provider shows data-driven model tiles and the selected password field', () => {
  const html = render();
  for (const [name, model] of [['Groq', 'whisper-large-v3-turbo'], ['OpenAI', 'gpt-transcribe'], ['xAI', 'grok-voice-transcribe-2.0'], ['ElevenLabs', 'scribe_v2'], ['Deepgram', 'nova-3'], ['Mistral', 'voxtral-mini-transcribe-3-0']]) {
    expect(html).toContain(name);
    expect(html).toContain(model);
  }
  expect(html.match(/id="provider-(?:groq-whisper-turbo|openai-gpt-4o-mini-transcribe|xai-stt|elevenlabs-scribe-v2|deepgram-nova-3|mistral-voxtral-3)"/g)).toHaveLength(6);
  expect(html).toMatch(/<input[^>]*id="provider-key"[^>]*type="password"/);
});

// Keep: compiled settings markup contains no phrasebook support flow for any provider.
for (const provider of providerOptions) test(`${provider.label} settings has no phrasebook support dialog`, () => {
  const html = render({ providerId: provider.id });
  expect(html).not.toContain('aria-label="Phrasebook support"');
  expect(html).not.toContain('ignores phrasebook');
});

test('Keychain presence configures an unchecked key; warnings appear on its tile and beside its empty password field', () => {
  const id = defaultSettings.providerId;
  const settings: Partial<EchoSettings> = {};
  expect(render(settings, { [id]: true })).toContain('Configured');
  expect(render(settings, { [id]: true })).toContain('Remove key');
  expect(render(settings, { [id]: false })).not.toMatch(/class="[^"]*remove-key/);
  expect(render({}, { [id]: true })).toMatch(/class="[^"]*remove-key/);
  expect(render({ ...settings, providerKeyStatuses: { [id]: 'valid' } }, { [id]: false })).not.toContain('Configured');
  const html = render({ ...settings, providerKeyStatuses: { [id]: 'invalid' } }, { [id]: true });
  expect(html).toMatch(/<input[^>]*id="provider-key"[^>]*type="password"/);
  expect(html).toContain('Paste your Groq key');
  expect(html.match(/aria-label="Credential needs attention"/g)).toHaveLength(2);
});


async function handlers(fail = false, platform = 'MacIntel') {
  let settings = { ...defaultSettings, language: 'es' }, draft = settings.pushToTalkShortcut;
  const patches: unknown[] = [], errors: unknown[] = [];
  const ctx = { settings: () => settings, hotkeyDraft: () => draft, setHotkeyDraft: (next: string) => { draft = next; },
    setActionError: (message: unknown) => errors.push(message),
    updateSettings: async (patch: object) => { if (fail) throw new Error('private detail'); patches.push(patch); return settings = { ...settings, ...patch }; } };
  const state = { capturingHotkey: false, savingSettings: false };
  const controller = createKeybindsController(ctx, state, {
    contains: (target) => target instanceof CaptureControl && target.inside,
    isEditable: (target) => target instanceof CaptureControl && !!target.editable,
    isChangeButton: (target) => target instanceof CaptureControl && target.inside
  }, platform);
  const actions = { ...controller,
    captureHotkey: (event: Partial<KeyboardEvent>) => controller.captureHotkey(event as KeyboardEvent),
    pointerdown: (event: { target: EventTarget | null; relatedTarget?: EventTarget | null }) => controller.cancelHotkeyCaptureOutside(event.target),
    focusout: (event: { relatedTarget: EventTarget | null; target?: EventTarget | null }) => { if (event.relatedTarget) controller.cancelHotkeyCaptureOutside(event.relatedTarget); }
  };
  return { actions, state, draft: () => draft, patches, errors, settings: () => settings };
}
const key = (value: string, code = value) => ({ key: value, code, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, preventDefault() {}, stopPropagation() {} });

class CaptureControl extends EventTarget {
  readonly inside: boolean;
  readonly editable: string;
  constructor(inside = true, editable = '') {
    super();
    this.inside = inside;
    this.editable = editable;
  }
}

test.each(['pointerdown', 'focusout'])('%s outside Shortcut ends capture and restores the saved keycap', async (event) => {
  const h = await handlers();
  h.actions.startHotkeyCapture();
  h.actions[event]({ target: new CaptureControl(false), relatedTarget: new CaptureControl(false, 'input') });
  expect(h.state.capturingHotkey).toBe(false);
  expect(h.draft()).toBe(defaultSettings.pushToTalkShortcut);
  await h.actions.captureHotkey(key('q', 'KeyQ'));
  expect(h.patches).toEqual([]);
});

test.each(['input', 'textarea', 'select', 'contenteditable'])('keydown in an outside %s never saves or consumes typing', async (editable) => {
  const h = await handlers();
  let prevented = false;
  h.actions.startHotkeyCapture();
  await h.actions.captureHotkey({ ...key('q', 'KeyQ'), target: new CaptureControl(false, editable), preventDefault() { prevented = true; } });
  expect(h.patches).toEqual([]);
  expect(prevented).toBe(false);
  expect(h.state.capturingHotkey).toBe(false);
  expect(h.draft()).toBe(defaultSettings.pushToTalkShortcut);
});

test('pointer and focus moving within Shortcut keep chord capture active', async () => {
  const h = await handlers();
  h.actions.startHotkeyCapture();
  h.actions.pointerdown({ target: new CaptureControl() });
  h.actions.focusout({ relatedTarget: new CaptureControl() });
  await h.actions.captureHotkey({ ...key('k', 'KeyK'), ctrlKey: true, target: new CaptureControl() });
  expect(h.patches).toEqual([{ pushToTalkShortcut: 'Control+K' }]);
});

test('blank space focusout with no related target keeps capture active for the pointer handler', async () => {
  const h = await handlers();
  h.actions.startHotkeyCapture();
  h.actions.pointerdown({ target: new CaptureControl() });
  h.actions.focusout({ relatedTarget: null });
  await h.actions.captureHotkey({ ...key('k', 'KeyK'), ctrlKey: true, target: new CaptureControl() });
  expect(h.patches).toEqual([{ pushToTalkShortcut: 'Control+K' }]);
});

test.each([['Enter', 'Enter'], [' ', 'Space']])('keyboard Cancel with %s leaves the saved shortcut unchanged', async (value, code) => {
  const h = await handlers();
  let prevented = false;
  h.actions.startHotkeyCapture();
  await h.actions.captureHotkey({ ...key(value, code), target: new CaptureControl(), preventDefault() { prevented = true; } });
  expect(h.patches).toEqual([]);
  expect(prevented).toBe(false);
  h.actions.cancelHotkeyCapture();
  await h.actions.captureHotkey(key('k', 'KeyK'));
  expect(h.patches).toEqual([]);
});

test.each([false, true])('Tab with shift=%s ends capture and permits focus traversal without saving', async (shiftKey) => {
  const h = await handlers();
  let prevented = false;
  h.actions.startHotkeyCapture();
  await h.actions.captureHotkey({ ...key('Tab'), shiftKey, target: new CaptureControl(), preventDefault() { prevented = true; } });
  expect(h.patches).toEqual([]);
  expect(prevented).toBe(false);
  await h.actions.captureHotkey(key('k', 'KeyK'));
  expect(h.patches).toEqual([]);
});

test('page navigation preserves focus; the rail permission action opens and focuses Settings', async () => {
  const calls: string[] = [];
  const { selectTab } = createDashboardController(initialDashboardState(), {
    tick: async () => {}, focusPermissions: () => {
      calls.push('focus:settings-permissions', 'scroll:settings-permissions');
    }
  });
  for (const tab of ['history', 'phrasebook', 'settings'] as const) selectTab(tab);
  await Promise.resolve();
  expect(calls).toEqual([]);
  for (const [tab, section] of [['permissions', 'permissions']] as const) {
    calls.length = 0;
    selectTab(tab);
    await Promise.resolve();
    expect(calls).toEqual([`focus:settings-${section}`, `scroll:settings-${section}`]);
  }
});

test('both Paste switches persist through settings updates and preserve unrelated settings', async () => {
  const h = await handlers();
  await h.actions.savePasteAfterTranscription(false);
  await h.actions.saveRestoreClipboard(false);
  expect(h.patches).toEqual([{ pasteAfterTranscription: false }, { restoreClipboard: false }]);
  expect(h.settings()).toMatchObject({ pasteAfterTranscription: false, restoreClipboard: false, language: 'es', providerId: defaultSettings.providerId });
});

test('Change captures a shortcut targeting its button; Escape on Cancel cancels without saving', async () => {
  const h = await handlers();
  h.actions.startHotkeyCapture(); await h.actions.captureHotkey({ ...key('Escape'), target: new CaptureControl() });
  await h.actions.captureHotkey(key('k', 'KeyK'));
  expect(h.patches).toEqual([]);
  h.actions.startHotkeyCapture(); h.actions.cancelHotkeyCapture();
  await h.actions.captureHotkey(key('x', 'KeyX'));
  expect(h.patches).toEqual([]);
  h.actions.startHotkeyCapture(); await h.actions.captureHotkey({ ...key('k', 'KeyK'), ctrlKey: true, target: new CaptureControl() });
  expect(h.patches).toEqual([{ pushToTalkShortcut: 'Control+K' }]);
});

test('a rejected Paste save retains committed settings and reports a fixed error', async () => {
  const h = await handlers(true);
  await h.actions.saveRestoreClipboard(false);
  expect(h.settings().restoreClipboard).toBe(true);
  expect(h.errors.at(-1)).toBe('Settings could not be saved. Try again.');
});

// Keep: public controls, native patch encoding, rejection recovery and saved-value parity.
test('Transcription renders named languages, saved tags and a multiline prompt without truncating pastes', () => {
  const html = render({ language: 'pt-BR', customPrompt: 'Use <plain> words.' });
  expect(html).toMatch(/<select[^>]*id="transcription-language"/);
  for (const [value, name] of [['', 'Auto-detect'], ['en', 'English'], ['es', 'Spanish'], ['pt', 'Portuguese'], ['fr', 'French'], ['de', 'German'], ['it', 'Italian'], ['nl', 'Dutch'], ['ja', 'Japanese'], ['ko', 'Korean'], ['zh', 'Chinese'], ['pt-BR', 'pt-BR']]) {
    expect(html).toMatch(new RegExp(`<option[^>]*value="${value}"[^>]*>${name}</option>`));
  }
  expect(html).toMatch(/<option[^>]*value="pt-BR"[^>]*selected/);
  expect(render()).toMatch(/<option[^>]*value=""[^>]*selected/);
  expect(html).toMatch(/<textarea[^>]*id="transcription-prompt"[^>]*rows="3"/);
  expect(html.match(/<textarea[^>]*id="transcription-prompt"[^>]*>/)![0]).not.toContain('maxlength');
  expect(html).toContain('Use &lt;plain> words.');
  expect(html).toContain('Style or context sent with every recording.');
});

async function transcriptionHandlers(initial: Partial<EchoSettings> = {}) {
  let settings = { ...defaultSettings, ...initial }, revision = 0;
  let rejection: unknown, hold: Promise<void> | undefined;
  const patches: Record<string, unknown>[] = [];
  const client = createNativeStorageClient({
    async invoke(_command, args) {
      patches.push(args!.patch as Record<string, unknown>);
      await hold;
      if (rejection) throw rejection;
      settings = { ...settings, ...args!.patch as object };
      return { previous: { settings, revision }, current: { settings, revision: ++revision }, changedFields: [] } as never;
    },
    async listen() { return () => {}; }
  });
  const ctx = { settings: () => settings, async updateSettings(patch: Partial<EchoSettings>) {
    settings = (await client.updateSettings(revision, patch)).current.settings;
    return settings;
  } };
  const state: TranscriptionState = { promptDraft: undefined, savingLanguage: false, savingPrompt: false, languageError: null, promptError: null };
  const controller = createTranscriptionController(ctx, state);
  const actions = { input: controller.editPrompt, change: controller.saveLanguage, blur: controller.savePrompt,
    prompt: controller.currentPrompt, promptError: () => state.promptError, languageError: () => state.languageError };
  return { actions, patches, settings: () => settings,
    reject: (error: unknown) => { rejection = error; }, hold: (promise: Promise<void>) => { hold = promise; } };
}
const fieldEvent = (value: string) => ({ currentTarget: { value } } as unknown as Event);

test('language selection and prompt blur persist and clear through the native settings client', async () => {
  const h = await transcriptionHandlers({ language: 'pt-BR', customPrompt: 'Saved context' });
  expect(h.actions.prompt()).toBe('Saved context');
  await h.actions.change(fieldEvent('es'));
  h.actions.input(fieldEvent('Use short sentences.\nKeep names.'));
  expect(h.patches).toEqual([{ language: 'es' }]);
  await h.actions.blur();
  await h.actions.change(fieldEvent(''));
  h.actions.input(fieldEvent(''));
  await h.actions.blur();
  expect(h.patches).toEqual([{ language: 'es' }, { customPrompt: 'Use short sentences.\nKeep names.' }, { language: null }, { customPrompt: null }]);
  expect(h.settings().language).toBeUndefined();
  expect(h.settings().customPrompt).toBeUndefined();
  await h.actions.blur();
  expect(h.patches).toHaveLength(4);
});

test('native validation errors retain an oversized draft and allow retry', async () => {
  const draft = 'é'.repeat(4001);
  const h = await transcriptionHandlers({ customPrompt: 'Saved' });
  h.reject('invalid storage field: customPrompt');
  h.actions.input(fieldEvent(draft));
  await h.actions.blur();
  expect(h.actions.prompt()).toBe(draft);
  expect(h.settings().customPrompt).toBe('Saved');
  expect(h.patches).toEqual([{ customPrompt: draft }]);
  expect(h.actions.promptError()).toBe('Too long. Accented and non-Latin characters count extra.');
  h.reject(new Error('invalid storage field: language'));
  await h.actions.change(fieldEvent('invalid_tag'));
  expect(h.actions.languageError()).toBe('Language must be a valid language tag, such as en or pt-BR.');
  expect(h.settings().language).toBeUndefined();
  h.reject(undefined);
  h.actions.input(fieldEvent('Corrected'));
  await h.actions.blur();
  expect(h.settings().customPrompt).toBe('Corrected');
  expect(h.actions.promptError()).toBeNull();
});

test('a pending prompt save preserves newer edits and a failed save hides private details', async () => {
  const h = await transcriptionHandlers();
  let release!: () => void;
  h.hold(new Promise<void>((resolve) => { release = resolve; }));
  h.actions.input(fieldEvent('First'));
  const saving = h.actions.blur();
  h.actions.input(fieldEvent('Newer'));
  release();
  await saving;
  expect(h.actions.prompt()).toBe('Newer');
  h.reject(new Error('private detail'));
  await h.actions.blur();
  expect(h.actions.prompt()).toBe('Newer');
  expect(h.actions.promptError()).toBe('Custom prompt could not be saved. Try again.');
});

// Keep: three native permission states, sequential grant emphasis and logging failure recovery.
const permissionState = (granted: boolean) => ({ granted, status: granted ? 'granted' : 'unknown_or_denied', message: undefined });
const permissions = (granted: boolean): DesktopPermissions => ({
  platform: 'macos',
  microphone: permissionState(granted), accessibility: permissionState(granted),
  input_monitoring: permissionState(granted)
});
test('Permissions shows three native states, lights the first pending Grant and keeps Globe separate', () => {
  const mixed = { ...permissions(false), microphone: permissionState(true) };
  const html = render({}, {}, mixed);
  const list = html.match(/<ul[^>]*aria-label="Desktop permissions"[^>]*>([\s\S]*?)<\/ul>/)![1];
  expect(list.match(/<li\b/g)).toHaveLength(3);
  expect(list.match(/>Granted</g)).toHaveLength(1);
  expect(list.match(/>Not granted</g)).toHaveLength(2);
  expect(list).not.toContain('Globe tap action');
  const grants = [...list.matchAll(/<button[^>]*aria-label="Grant ([^"]+)"[^>]*>/g)];
  expect(grants.map((match) => match[1])).toEqual(['Accessibility', 'Input Monitoring']);
  expect(grants[0][0]).toContain('echo-lit');
  expect(grants[1][0]).not.toContain('echo-lit');
  expect(html).toContain('1/3 granted');
  expect(html).not.toContain('Use Fn for Echo'); // Hidden until the native macOS result arrives.
  expect(html).toContain('Re-check');
  expect(render({}, {}, permissions(true))).toContain('3/3 granted');
  expect(render()).not.toContain('>Granted<');
  expect(render({ debugLoggingEnabled: true })).toMatch(/<button[^>]*role="switch"[^>]*aria-checked="true"[^>]*aria-labelledby="debug-log-label"/);
});

function permissionHandlers(ctx: object, bindings: object = {}) {
  return createPermissionsController(ctx as Parameters<typeof createPermissionsController>[0], { permissionRefreshState: 'idle', permissionAction: undefined }, {
    tick: async () => {}, activeElement: () => null, focusHeading() {}, refreshFn: async () => {}, ...bindings
  });
}
// Keep: unknown native consent remains visible and offers the OS Settings action.
test('unknown microphone consent needs attention with a Settings action', async () => {
  const native = { ...permissions(true), microphone: { granted: false, status: 'unknown', message: 'Check microphone settings.' } };
  const html = render({}, {}, native);
  const list = html.match(/<ul[^>]*aria-label="Desktop permissions"[^>]*>([\s\S]*?)<\/ul>/)![1];
  expect(list).toContain('Unknown');
  expect(list).toMatch(/aria-label="Settings Microphone"[^>]*>Settings</);
  expect(html).toContain('2/3 granted');
  expect(html).not.toContain('Permissions granted');
  const calls: string[] = [];
  const feedback: { error: string | null } = { error: null };
  const actions = permissionHandlers({
    setActionError(value: string | null) { feedback.error = value; },
    runtime: () => ({ async openDesktopPermissionSettings(kind: string) { calls.push(kind); throw new Error('native shell failed'); } })
  });
  await actions.openPermissionSettings('microphone');
  expect(calls).toEqual(['microphone']);
  expect(feedback.error).toBe('Could not open System Settings. Try again.');
});
// Keep: the Settings action shares the permission panel's pending guard.
test('microphone Settings rejects repeated actions until opening completes', async () => {
  let calls = 0;
  let release!: () => void;
  const opening = new Promise<void>((resolve) => { release = resolve; });
  const actions = permissionHandlers({
    setActionError() {},
    runtime: () => ({ async openDesktopPermissionSettings() { calls++; await opening; } })
  });
  const pending = actions.openPermissionSettings('microphone');
  const repeated = actions.openPermissionSettings('microphone');
  release();
  await Promise.all([pending, repeated]);
  expect(calls).toBe(1);
});
test.each([false, true])('Grant and Re-check use native results with microphone granted=%s', async (granted) => {
  let current = permissions(false), next = permissions(granted);
  const calls: string[] = [];
  const ctx = { permissions: () => current, permissionCheckState: () => 'ready', setActionError() {}, setPermissions: (value: DesktopPermissions) => { current = value; },
    async refreshPermissions() { calls.push('check'); current = next; },
    runtime: () => ({ async requestDesktopPermission(kind: string) { calls.push(kind); return next; },
      async openDesktopPermissionSettings() { calls.push('settings'); } }) };
  const actions = permissionHandlers(ctx);
  await actions.requestPermission('microphone');
  await actions.openPermissionSettings('accessibility');
  expect(current.microphone.granted).toBe(granted);
  expect(current.accessibility.granted).toBe(granted);
  next = permissions(true);
  await actions.doRefresh();
  expect(current).toBe(next);
  expect(calls).toEqual(['microphone', ...(!granted ? ['settings'] : []), 'settings', 'check']);
});
test('Debug log saves before attaching, detaches on off, shows the native path and recovers from failures', async () => {
  const calls: unknown[] = [];
  let failure = false;
  const state: DebugLogState = { diagnosticStatus: { state: 'off', message: '' }, logPath: undefined, logPathState: 'loading', actionMessage: '', isSaving: false };
  const controller = createDebugLogController({
    async updateSettings(patch: object) { calls.push(patch); if (failure) throw new Error('private native detail'); return defaultSettings; }
  }, state, {
    async loadContext() { if (failure) throw new Error('private path'); return { logFilePath: '/native/logs/Echo.log' }; },
    subscribeStatus: () => () => {},
    async attach() { calls.push('attach'); }, async detach() { calls.push('detach'); }
  });
  const actions = { ...controller, path: () => state.logPath, pathState: () => state.logPathState, error: () => state.actionMessage };
  await actions.loadDiagnosticContext();
  expect(actions.path()).toBe('/native/logs/Echo.log');
  expect(actions.pathState()).toBe('ready');
  await actions.saveDiagnosticsSettings({ debugLoggingEnabled: true });
  await actions.saveDiagnosticsSettings({ debugLoggingEnabled: false });
  expect(calls).toEqual([{ debugLoggingEnabled: true }, 'attach', { debugLoggingEnabled: false }, 'detach']);
  failure = true;
  await actions.loadDiagnosticContext();
  expect(actions.path()).toBeUndefined();
  expect(actions.pathState()).toBe('unavailable');
  await actions.saveDiagnosticsSettings({ debugLoggingEnabled: true });
  expect(actions.error()).toBe('Could not change detailed logging. Try again.');
  expect(calls.at(-1)).toEqual({ debugLoggingEnabled: true });
});

// Keep: malformed native responses cannot blank Settings or invent denied permissions.
test.each(['microphone', 'accessibility', 'input_monitoring'] as const)('missing %s remains unknown without breaking Settings or the rail', (kind) => {
  const partial = permissions(true);
  delete (partial as Partial<DesktopPermissions>)[kind];
  const html = render({}, {}, partial);
  expect(html).toContain('Debug log');
  expect(html).not.toContain('3/3 granted');
  expect(html).toContain('Unknown');
  expect(html).not.toContain('Permissions granted');
  expect(html).not.toMatch(/aria-label="Grant /);
});
test.each([['checking', 'Checking…'], ['unknown', 'Unknown']] as const)('%s permission results offer no Grant or count', (state, label) => {
  const html = render({}, {}, undefined, state);
  const list = html.match(/<ul[^>]*aria-label="Desktop permissions"[^>]*>([\s\S]*?)<\/ul>/)![1];
  expect(list.match(new RegExp(`>${label}<`, 'g'))).toHaveLength(3);
  expect(list).not.toContain('Not granted');
  expect(list).not.toMatch(/aria-label="Grant /);
  expect(html).not.toMatch(/\d\/3 granted/);
});
test.each(['denied', 'failed', 'partial', 'missing'] as const)('native permission refresh keeps History selected through %s and recovery', async (outcome) => {
  const state = initialDashboardState({ permissions: permissions(true), permissionCheckState: 'ready' });
  let release!: () => void;
  let recover = false;
  state.runtime = { async getDesktopPermissions() {
    await new Promise<void>((resolve) => { release = resolve; });
    if (recover) return permissions(true);
    if (outcome === 'failed') throw new Error('private');
    if (outcome === 'missing') return undefined;
    if (outcome === 'partial') return { microphone: permissionState(false) };
    return permissions(false);
  } } as unknown as EchoRuntime;
  const shell = createDashboardController(state, { tick: async () => {}, focusPermissions() {} });
  const pending = shell.context.refreshPermissions();
  expect(state.permissionCheckState).toBe('checking');
  release(); await pending;
  expect(state.settingsTab).toBe('history');
  expect(state.permissionCheckState).toBe(['failed', 'missing'].includes(outcome) ? 'unknown' : 'ready');
  expect(state.permissions).toEqual(outcome === 'denied' ? permissions(false) : outcome === 'partial' ? { microphone: permissionState(false) } as DesktopPermissions : undefined);
  if (outcome === 'failed') expect(state.actionError).toBe('Permission check failed. Re-check to try again.');
  recover = true;
  const retry = shell.context.refreshPermissions(); release(); await retry;
  expect(state.permissions).toEqual(permissions(true));
  expect(state.permissionCheckState).toBe('ready');
  expect(state.settingsTab).toBe('history');
});

// Keep: native disabled buttons lose keyboard focus while aria-disabled stays focusable.
test('Grant and Re-check stay focusable while their handlers reject repeat activation', async () => {
  const html = render({}, {}, permissions(false));
  for (const button of html.match(/<button[^>]*(?:class="recheck|aria-label="Grant )[^>]*>/g) ?? []) {
    expect(button).toContain('aria-disabled="false"');
    expect(button).not.toMatch(/\sdisabled(?:[\s=>])/);
  }
  let release!: () => void;
  let requests = 0;
  let hold: Promise<void>;
  const ctx = { permissions: () => permissions(false), permissionCheckState: () => 'ready', setActionError() {}, setPermissions() {},
    refreshPermissions: async () => { requests++; await hold; },
    runtime: () => ({ requestDesktopPermission: async () => { requests++; await hold; return permissions(true); } }) };
  const actions = permissionHandlers(ctx);
  for (const action of [() => actions.doRefresh(), () => actions.requestPermission('microphone')]) {
    hold = new Promise<void>((resolve) => { release = resolve; });
    const pending = action();
    const recheck = actions.doRefresh();
    const grant = actions.requestPermission('microphone');
    release(); await Promise.all([pending, recheck, grant]);
  }
  expect(requests).toBe(2);
});
test('the rail announces unknown permissions after a failed check, even with stale grants', () => {
  const html = render({}, {}, permissions(true), 'unknown');
  expect(html).toContain('Permissions unknown');
  expect(html).not.toContain('Permissions granted');
  expect(html).not.toContain('Checking permissions');
});

test('a successful Grant moves focus to Permissions when its focused action disappears', async () => {
  const trigger = { isConnected: true };
  let focused = false;
  const actions = permissionHandlers({
    permissions: () => permissions(false), permissionCheckState: () => 'ready', setActionError() {},
    setPermissions() { trigger.isConnected = false; },
    runtime: () => ({ requestDesktopPermission: async () => permissions(true) })
  }, { activeElement: () => trigger, focusHeading() { focused = true; } });
  await actions.requestPermission('microphone', trigger as HTMLButtonElement);
  expect(focused).toBe(true);
});

// Merge: settings_changed status, presence and draft parity use one controller matrix.
test.each(['invalid', 'valid', 'failure'] as const)('native settings reload preserves drafts and refreshes presence once (%s)', async (status) => {
  const failure = status === 'failure';
  const state = initialDashboardState();
  state.apiKeyDrafts[defaultSettings.providerId] = 'synthetic-draft';
  state.hotkeyDraft = 'Control+K';
  let reads = 0;
  const saved = { ...defaultSettings, providerKeyStatuses: { [defaultSettings.providerId]: failure ? 'valid' as const : status } };
  state.runtime = {
    async getSettings() { if (failure) throw new Error('private storage detail'); return saved; },
    async getProviderApiKeyPresence() { reads++; return { [defaultSettings.providerId]: true }; }
  } as unknown as EchoRuntime;
  const shell = createDashboardController(state, { tick: async () => {}, focusPermissions() {} });
  shell.handleRuntimeEvent({ type: 'settings_changed' });
  for (let i = 0; i < 5; i++) await Promise.resolve();
  expect(state.settings).toEqual(failure ? defaultSettings : saved);
  expect(state.apiKeyDrafts[defaultSettings.providerId]).toBe('synthetic-draft');
  expect(state.hotkeyDraft).toBe('Control+K');
  expect(reads).toBe(failure ? 0 : 1);
  expect(state.actionError).toBe(failure ? 'Settings could not be refreshed. Try again.' : null);
});

// Keep: the panel header uses the Fn row's refresh path and suppresses concurrent re-checks.
test('Permissions header re-check refreshes the Fn row once while pending', async () => {
  let checks = 0, fnReads = 0, release!: () => void;
  const actions = permissionHandlers({ refreshPermissions: async () => { checks++; } }, {
    refreshFn: async () => { fnReads++; await new Promise<void>((resolve) => { release = resolve; }); }
  });
  const pending = actions.doRefresh();
  await actions.doRefresh();
  expect([checks, fnReads]).toEqual([1, 1]);
  release(); await pending;
});

// Keep: diagnostic status subscription is owned by the panel mount, and pending saves reject repetition.
test('Debug log mount loads its path, follows native status and removes its listener', async () => {
  const state: DebugLogState = { diagnosticStatus: { state: 'off', message: '' }, logPath: undefined,
    logPathState: 'loading', actionMessage: '', isSaving: false };
  let listener!: (status: DebugLogState['diagnosticStatus']) => void;
  let removed = false, saves = 0, attaches = 0, release!: () => void;
  const controller = createDebugLogController({
    async updateSettings() { saves++; await new Promise<void>((resolve) => { release = resolve; }); return defaultSettings; }
  }, state, {
    loadContext: async () => ({ logFilePath: '/native/logs/Echo.log' }),
    subscribeStatus(next) { listener = next; return () => { removed = true; }; },
    attach: async () => { attaches++; }, detach: async () => {}
  });
  const cleanup = controller.mount();
  await Promise.resolve();
  expect(state.logPath).toBe('/native/logs/Echo.log');
  listener({ state: 'error', message: 'Unavailable' });
  expect(state.diagnosticStatus).toEqual({ state: 'error', message: 'Unavailable' });
  const pending = controller.saveDiagnosticsSettings({ debugLoggingEnabled: true });
  await controller.saveDiagnosticsSettings({ debugLoggingEnabled: true });
  expect([saves, attaches]).toEqual([1, 0]);
  release(); await pending;
  expect([saves, attaches]).toEqual([1, 1]);
  cleanup();
  expect(removed).toBe(true);
});

// Keep: the header owns permission pending, independently of the Fn read.
test.each([false, true])('Permissions re-check settles before a delayed Fn read (failure: %s)', async (failure) => {
  let finishPermissions!: () => void, finishFn!: () => void;
  const permission = new Promise<void>(resolve => { finishPermissions = resolve; });
  const fn = new Promise<void>(resolve => { finishFn = resolve; });
  const state = { permissionRefreshState: 'idle' as 'idle' | 'checking', permissionAction: undefined };
  const controller = createPermissionsController({
    refreshPermissions: async () => { await permission; if (failure) throw new Error('check failed'); },
    runtime: () => undefined, setActionError() {}, setPermissions() {}
  }, state, { tick: async () => {}, activeElement: () => null, focusHeading() {}, refreshFn: () => fn });
  const pending = controller.doRefresh().catch(() => {});
  expect(state.permissionRefreshState).toBe('checking');
  finishPermissions();
  for (let i = 0; i < 10; i++) await Promise.resolve();
  try { expect(state.permissionRefreshState).toBe('idle'); }
  finally { finishFn(); await pending; }
});

// Keep: the saved accelerator must preserve the physical OS modifier.
test.each([['Win32', 'Super+Alt+F9'], ['MacIntel', 'CommandOrControl+Alt+F9']])('recorder preserves Meta on %s', async (platform, expected) => {
  const h = await handlers(false, platform);
  h.actions.startHotkeyCapture();
  await h.actions.captureHotkey({ ...key('F9', 'F9'), metaKey: true, altKey: true });
  expect(h.patches).toEqual([{ pushToTalkShortcut: expected }]);
});

// Keep: Windows permission rows describe requirements, not hook availability.
test('healthy Windows permission payload has no degraded shortcut copy', () => {
  expect(render({ pushToTalkShortcut: 'Control+Windows' }, {}, windowsPermissions)).not.toContain('Native shortcut unavailable.');
});

// Keep: only a known hook failure shows degraded instructions, with a usable accelerator.
test('Shortcut uses native availability and formats the Windows fallback', () => {
  const html = renderKeybinds(false);
  expect(html).toContain('Native shortcut unavailable.');
  expect(html).toContain('Fallback: Ctrl+Shift+Space.');
  expect(html).toContain('Press once to record, then press again to transcribe.');
  expect(html).not.toContain('Hold this key to record');
  for (const available of [true, undefined, null]) expect(renderKeybinds(available)).not.toContain('Native shortcut unavailable.');
});

// Keep: fresh installs expose the sole credential path directly.
test('fresh Provider offers a password without credential-source controls', () => {
  const html = render();
  expect(html).toMatch(/<input[^>]*id="provider-key"[^>]*type="password"/);
  expect(html).not.toContain('Where the key lives');
  expect(html).not.toContain('Environment variable');
  expect(html).not.toContain('provider-env');
});

// Keep: Background markup matrix over platform copy, labels and every saved combination, including a fresh install.
const backgroundLabels = { runInBackground: 'Keep Echo running when the dashboard closes', launchAtLogin: 'Start Echo at login' } as const;
test.each([
  ['macos', permissions(true), 'Echo stays in the menu bar. Quit from there or with Cmd+Q.'],
  ['windows', windowsPermissions, 'Echo stays in the notification area. Quit from its icon.']
] as const)('Background on %s shows platform help and each saved switch combination', (_platform, native, keepRunningHelp) => {
  for (const [runInBackground, launchAtLogin] of [[false, false], [true, false], [false, true], [true, true]]) {
    const html = render({ runInBackground, launchAtLogin }, {}, native);
    const card = html.match(/<section[^>]*aria-labelledby="settings-background"[^>]*>([\s\S]*?)<\/section>/)![1];
    expect(card).toMatch(/<h2[^>]*>Background<\/h2>/);
    expect([...card.matchAll(/<p[^>]*>([^<]+)<\/p>/g)].map((match) => match[1])).toEqual([keepRunningHelp, 'Opens Echo when you sign in.']);
    for (const [field, checked] of [['runInBackground', runInBackground], ['launchAtLogin', launchAtLogin]] as const) {
      const button = card.match(new RegExp(`<button[^>]*aria-label="${backgroundLabels[field]}"[^>]*>`))![0];
      expect(button).toContain('role="switch"');
      expect(button).toContain(`aria-checked="${checked}"`);
      expect(button).toContain('aria-disabled="false"');
    }
  }
  const fresh = render({}, {}, native);
  for (const label of Object.values(backgroundLabels)) expect(fresh).toMatch(new RegExp(`<button[^>]*aria-checked="false"[^>]*aria-label="${label}"`));
});

// Keep: the intact Background card commits through the native settings client and reads
// login-item status through the Tauri invoke boundary; these are the only commands it sends.
const STATUS = 'get_launch_at_login_status', UPDATE = 'update_persisted_settings';
type BackgroundOptions = {
  initial?: Partial<EchoSettings>;
  native?: DesktopPermissions;
  status?: (saved: EchoSettings) => Promise<{ registered: boolean }>;
};
async function mountBackground(options: BackgroundOptions = {}) {
  const harness = await clientHarness(`<script>
    import BackgroundPanel from '${sourcePath('src/lib/ui/dashboard/panels/BackgroundPanel.svelte')}';
    import { provideDashboardContext } from '${sourcePath('src/lib/ui/dashboard/context.ts')}';
    let { client, native, initial } = $props();
    let settings = $state(initial), alert = $state(null), revision = 0;
    provideDashboardContext({
      settings: () => settings, permissions: () => native, setActionError: (message) => { alert = message; },
      async updateSettings(patch) {
        const commit = await client.updateSettings(revision, patch);
        revision = commit.current.revision;
        return settings = commit.current.settings;
      }
    });
    let shown = $state(true);
    export const view = { alert: () => alert, settings: () => settings, show: (next) => { shown = next; } };
  </script>{#if shown}<BackgroundPanel />{/if}`);
  const commands: string[] = [], patches: unknown[] = [];
  let saved: EchoSettings = { ...defaultSettings, language: 'es', ...options.initial }, revision = 0;
  let hold: Promise<void> | undefined, rejection: unknown;
  let status = options.status ?? (async (current: EchoSettings) => ({ registered: current.launchAtLogin }));
  harness.install('isTauri', true);
  (harness.window as any).__TAURI_INTERNALS__ = { invoke: (command: string) => {
    commands.push(command);
    if (command !== STATUS) throw new Error(`unexpected ${command}`);
    return status(saved);
  } };
  const client = createNativeStorageClient({
    async invoke(command, args) {
      commands.push(command);
      patches.push(args!.patch);
      await hold;
      if (rejection) throw rejection;
      const previous = { settings: saved, revision };
      saved = { ...saved, ...args!.patch as object };
      return { previous, current: { settings: saved, revision: ++revision }, changedFields: Object.keys(args!.patch as object) } as never;
    },
    async listen() { return () => {}; }
  });
  const { view } = harness.mount({ client, native: options.native ?? permissions(true), initial: saved }) as { view: { alert(): string | null; settings(): EchoSettings; show(next: boolean): void } };
  const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); harness.flush(); };
  await settle();
  const switches = (label: keyof typeof backgroundLabels) => harness.target.querySelector(`[aria-label="${backgroundLabels[label]}"]`) as unknown as HTMLButtonElement;
  async function press(label: keyof typeof backgroundLabels) {
    switches(label).focus();
    switches(label).click();
    harness.settleEvents();
    await settle();
  }
  function holdSaves() {
    let release!: () => void;
    hold = new Promise<void>((resolve) => { release = resolve; });
    return async () => { release(); hold = undefined; await settle(); };
  }
  // Every line the card shows under its switch labels, in order.
  const lines = () => [...harness.target.querySelectorAll('section p')].map((line) => ({ text: line.textContent, alert: line.getAttribute('role') === 'alert' }));
  return { harness, view, commands, patches, switches, press, holdSaves, settle, lines,
    reject: (error: unknown) => { rejection = error; },
    setStatus: (next: typeof status) => { status = next; } };
}

function deferredStatus() {
  let resolve!: (value: { registered: boolean }) => void;
  const promise = new Promise<{ registered: boolean }>((done) => { resolve = done; });
  return { promise, resolve };
}

const macHelp = 'Echo stays in the menu bar. Quit from there or with Cmd+Q.';
const helpLines = [{ text: macHelp, alert: false }, { text: 'Opens Echo when you sign in.', alert: false }];

test('Background switches enable and disable one setting at a time, keep focus while pending and send only settings commits', async () => {
  const h = await mountBackground();
  try {
    expect(h.commands).toEqual([STATUS]);
    const release = h.holdSaves();
    await h.press('runInBackground');
    for (const label of ['runInBackground', 'launchAtLogin'] as const) {
      expect(h.switches(label).getAttribute('aria-disabled')).toBe('true');
      expect(h.switches(label).hasAttribute('disabled')).toBe(false);
    }
    expect(h.harness.window.document.activeElement).toBe(h.switches('runInBackground') as never);
    await h.press('runInBackground');
    await h.press('launchAtLogin');
    expect(h.patches).toEqual([{ runInBackground: true }]);
    await release();
    expect(h.switches('runInBackground').getAttribute('aria-checked')).toBe('true');
    expect(h.switches('runInBackground').getAttribute('aria-disabled')).toBe('false');
    await h.press('launchAtLogin');
    await h.press('runInBackground');
    expect(h.patches).toEqual([{ runInBackground: true }, { launchAtLogin: true }, { runInBackground: false }]);
    expect(h.view.settings()).toEqual({ ...defaultSettings, language: 'es', runInBackground: false, launchAtLogin: true });
    expect([h.switches('runInBackground').getAttribute('aria-checked'), h.switches('launchAtLogin').getAttribute('aria-checked')]).toEqual(['false', 'true']);
    expect(h.commands).toEqual([STATUS, UPDATE, UPDATE, STATUS, UPDATE]);
  } finally { await h.harness.cleanup(); }
});

test('a rejected Background save keeps committed switches, reports the shared alert and clears it on the next save', async () => {
  const h = await mountBackground();
  try {
    await h.press('launchAtLogin');
    h.reject(new Error('private native detail'));
    await h.press('launchAtLogin');
    expect(h.view.alert()).toBe('Settings could not be saved. Try again.');
    expect(h.view.settings().launchAtLogin).toBe(true);
    expect(h.switches('launchAtLogin').getAttribute('aria-checked')).toBe('true');
    h.reject(undefined);
    const release = h.holdSaves();
    await h.press('launchAtLogin');
    expect(h.view.alert()).toBeNull();
    await release();
    expect(h.switches('launchAtLogin').getAttribute('aria-checked')).toBe('false');
    expect(h.patches).toEqual([{ launchAtLogin: true }, { launchAtLogin: false }, { launchAtLogin: false }]);
    expect(h.commands).toEqual([STATUS, UPDATE, STATUS, UPDATE, STATUS, UPDATE, STATUS]);
  } finally { await h.harness.cleanup(); }
});

// Keep: Settings unmounts on navigation, and the pending guard must outlive the card.
test('a pending Background save stays guarded after Settings unmounts and mounts again', async () => {
  const h = await mountBackground();
  try {
    const release = h.holdSaves();
    await h.press('runInBackground');
    h.view.show(false); h.harness.flush();
    expect(h.harness.target.querySelector('[role="switch"]')).toBeNull();
    h.view.show(true); h.harness.flush();
    for (const label of ['runInBackground', 'launchAtLogin'] as const) {
      expect(h.switches(label).getAttribute('aria-disabled')).toBe('true');
      await h.press(label);
    }
    expect(h.commands).toEqual([STATUS, UPDATE, STATUS]);
    await release();
    expect([h.switches('runInBackground').getAttribute('aria-checked'), h.switches('launchAtLogin').getAttribute('aria-checked')]).toEqual(['true', 'false']);
    for (const label of ['runInBackground', 'launchAtLogin'] as const) expect(h.switches(label).getAttribute('aria-disabled')).toBe('false');
    expect(h.patches).toEqual([{ runInBackground: true }]);
  } finally { await h.harness.cleanup(); }
});

// Keep: a refused registration keeps the committed switch, shows the card's own alert
// with the native detail instead of the shared one, and the next save clears it (AC3, AC4).
test.each([['on', false], ['off', true]] as const)('a refused login registration shows "could not be turned %s" in the card', async (verb, start) => {
  const h = await mountBackground({ initial: { launchAtLogin: start } });
  try {
    h.reject('login_item_failed: Permission denied (os error 13)');
    await h.press('launchAtLogin');
    expect(h.lines()).toEqual([...helpLines, { text: `Start Echo at login could not be turned ${verb}: Permission denied (os error 13)`, alert: true }]);
    expect(h.view.alert()).toBeNull();
    expect(h.view.settings().launchAtLogin).toBe(start);
    expect(h.switches('launchAtLogin').getAttribute('aria-checked')).toBe(String(start));
    expect(h.commands).toEqual([STATUS, UPDATE, STATUS]);
    h.reject(undefined);
    await h.press('launchAtLogin');
    expect(h.lines()).toEqual(helpLines);
    expect(h.switches('launchAtLogin').getAttribute('aria-checked')).toBe(String(!start));
  } finally { await h.harness.cleanup(); }
});

// Keep: the card alert reads as an error and wraps long native detail, while the mismatch
// line keeps the muted help style. Happy-dom resolves the cascade against the card's
// own paragraph rule; rendered colour still needs the running app.
test('the login alert uses the error tone and wraps while the mismatch line stays help text', async () => {
  const panel = fileURLToPath(new URL('../src/lib/ui/dashboard/panels/BackgroundPanel.svelte', import.meta.url));
  const css = compile(await Bun.file(panel).text(), { filename: panel, css: 'external' }).css!.code;
  const h = await mountBackground({ status: async () => ({ registered: true }) });
  try {
    h.reject('login_item_failed: Permission denied (os error 13)');
    await h.press('launchAtLogin');
    const style = h.harness.window.document.createElement('style');
    style.textContent = `:root { --tone-error-text: rgb(255, 0, 0); --echo-fg-3: rgb(0, 0, 255); }\n${css}`;
    h.harness.window.document.head.append(style);
    const looks = [...h.harness.target.querySelectorAll('section p')].slice(1).map((line) => {
      const computed = h.harness.window.getComputedStyle(line);
      return [line.textContent, line.getAttribute('role'), computed.color, computed.overflowWrap];
    });
    expect(looks).toEqual([
      ['Opens Echo when you sign in.', null, 'rgb(0, 0, 255)', ''],
      ['macOS still starts Echo at login. Turn the switch on and off again.', null, 'rgb(0, 0, 255)', ''],
      ['Start Echo at login could not be turned on: Permission denied (os error 13)', 'alert', 'rgb(255, 0, 0)', 'anywhere']
    ]);
  } finally { await h.harness.cleanup(); }
});

// Keep: the mismatch line appears only when the OS and the saved setting disagree, names
// the platform, and stays hidden when the status read fails (AC9).
test.each([
  ['macos', permissions(true), 'macOS'],
  ['windows', windowsPermissions, 'Windows']
] as const)('Background on %s shows a mismatch line only when the OS disagrees with the saved setting', async (_platform, native, os) => {
  const help = native === windowsPermissions ? 'Echo stays in the notification area. Quit from its icon.' : macHelp;
  for (const [saved, registered, line] of [
    [true, false, `Not registered with ${os}. Turn the switch off and on again.`],
    [false, true, `${os} still starts Echo at login. Turn the switch on and off again.`],
    [true, true, null],
    [false, false, null],
    [true, 'fails', null],
    [false, 'fails', null]
  ] as const) {
    const h = await mountBackground({ initial: { launchAtLogin: saved }, native,
      status: async () => { if (registered === 'fails') throw new Error('unavailable'); return { registered }; } });
    try {
      expect(h.lines().map((entry) => entry.text)).toEqual([help, 'Opens Echo when you sign in.', ...(line ? [line] : [])]);
      expect(h.lines().some((entry) => entry.alert)).toBe(false);
    } finally { await h.harness.cleanup(); }
  }
});

// Keep: turning the switch off and on re-reads status after each save, so the line clears,
// and the stale status never shows a wrong line while the re-read is pending (AC10).
test('turning Start Echo at login off and on clears the mismatch line once registration succeeds', async () => {
  const h = await mountBackground({ initial: { launchAtLogin: true }, status: async () => ({ registered: false }) });
  try {
    expect(h.lines().at(-1)!.text).toBe('Not registered with macOS. Turn the switch off and on again.');
    await h.press('launchAtLogin');
    expect(h.lines()).toEqual(helpLines);
    const postSaveRead = deferredStatus();
    h.setStatus(() => postSaveRead.promise);
    await h.press('launchAtLogin');
    expect(h.lines()).toEqual(helpLines);
    postSaveRead.resolve({ registered: true });
    await h.settle();
    expect(h.lines()).toEqual(helpLines);
    expect(h.commands).toEqual([STATUS, UPDATE, STATUS, UPDATE, STATUS]);
  } finally { await h.harness.cleanup(); }
});

// Keep: a status read older than the newest one issued, or resolving after the card
// unmounts, never changes the card (status ordering).
test('a delayed mount status read does not restore a mismatch after a save', async () => {
  const mountRead = deferredStatus();
  const h = await mountBackground({ status: () => mountRead.promise });
  try {
    h.setStatus(async () => ({ registered: true }));
    await h.press('launchAtLogin');
    expect(h.lines()).toEqual(helpLines);
    mountRead.resolve({ registered: false });
    await h.settle();
    expect(h.lines()).toEqual(helpLines);
  } finally { await h.harness.cleanup(); }
});

test('a status read resolving after the card unmounts changes nothing', async () => {
  const firstRead = deferredStatus();
  const h = await mountBackground({ initial: { launchAtLogin: true }, status: () => firstRead.promise });
  try {
    h.view.show(false); h.harness.flush();
    firstRead.resolve({ registered: false });
    await h.settle();
    h.setStatus(() => new Promise(() => {}));
    h.view.show(true); h.harness.flush();
    await h.settle();
    expect(h.lines()).toEqual(helpLines);
    expect(h.commands).toEqual([STATUS, STATUS]);
  } finally { await h.harness.cleanup(); }
});
