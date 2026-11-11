import type { EchoRuntime } from '../src/lib/core/types';
import { createDashboardController, initialDashboardState } from '../src/lib/ui/dashboard/dashboard-controller';
import { createSetupController } from '../src/lib/ui/dashboard/panels/setup-controller';
import { beforeAll, expect, test } from 'bun:test';
import { compileRenderer } from './helpers/render-svelte';
import { defaultSettings, providerOptions } from '../src/lib/core/settings';
import type { ProviderId } from '../src/lib/core/settings';

// Keep: routing and compiled markup through simulated native results. Browser gates use the host harness.
const settle = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };

async function shell(present?: ProviderId) {
  let presence = Object.fromEntries(providerOptions.map((p) => [p.id, p.id === present]));
  let settings = { ...defaultSettings };
  const calls: string[] = [];
  let fail = false, hold: Promise<Record<string, boolean>> | undefined;
  let notify!: (event: object) => void;
  const runtime = {
    subscribe(fn: typeof notify) { notify = fn; return () => {}; },
    async getSettings() { calls.push('settings'); return settings; },
    async updateSettings(patch: Partial<typeof settings>) {
      calls.push('update'); settings = { ...settings, ...patch };
      notify({ type: 'settings_changed' });
      return settings;
    },
    async getSnapshot() { return { state: 'idle' }; },
    async listHistory() { return []; },
    async getDesktopPermissions() { return { microphone: { granted: false } }; },
    async getProviderApiKeyPresence() { calls.push('presence'); if (fail) throw new Error('private native detail'); return hold ?? presence; },
    async probeProviderKey() { calls.push('probe'); },
    async setProviderApiKey() { calls.push('store'); }
  };
  const state = initialDashboardState();
  const controller = createDashboardController(state, { tick: async () => {}, focusPermissions() {} });
  const actions = { tab: () => state.settingsTab };
  return { actions, mount: () => controller.mount(() => runtime as unknown as EchoRuntime), calls, ctx: controller.context, notify: (event: object) => notify(event),
    presence(value: typeof presence) { presence = value; }, fail(value: boolean) { fail = value; },
    hold(value?: typeof hold) { hold = value; } };
}

test('any configured provider keeps History home despite missing permissions', async () => {
  const h = await shell(providerOptions[1].id);
  h.mount(); await settle();
  expect(h.actions.tab()).toBe('history');
  expect(h.ctx.setupRequired()).toBe(false);
  expect(h.calls).toEqual(['settings', 'presence']);
});

test('a provider status settings commit refreshes native presence once through its event', async () => {
  const h = await shell(); h.mount(); await settle();
  h.calls.length = 0;
  await h.ctx.updateSettings({ providerKeyStatuses: { [defaultSettings.providerId]: 'valid' } });
  await settle();
  expect(h.calls).toEqual(['update', 'settings', 'presence']);
});

test('no native credential opens setup once; browsing and finish remain available without a provider', async () => {
  const h = await shell();
  expect(h.ctx.setupRequired()).toBe(false); // Unknown is not an empty native result.
  h.mount(); await settle();
  expect(h.actions.tab()).toBe('setup');
  expect(h.ctx.setupRequired()).toBe(true);
  for (const page of ['history', 'phrasebook', 'settings'] as const) {
    h.ctx.setActiveTab(page);
    await h.ctx.refreshProviderKeyPresence();
    expect(h.actions.tab()).toBe(page);
  }
  h.ctx.setActiveTab('setup');
  const settings = h.ctx.settings();
  const state = { step: 1 };
  let focuses = 0;
  const controller = createSetupController(h.ctx, state, { tick: async () => {}, focusHeading: () => { focuses++; } });
  const flow = { ...controller, step: () => state.step };
  await flow.setStep(2); expect(flow.step()).toBe(2);
  await flow.setStep(1); expect(flow.step()).toBe(1);
  await flow.setStep(2); flow.finishSetup();
  expect(focuses).toBe(3);
  expect(h.actions.tab()).toBe('history');
  expect(h.ctx.settings()).toEqual(settings);
  expect(h.calls).not.toContain('probe');
});

test('presence refresh removes and restores quiet entries without reopening setup or clearing drafts', async () => {
  const h = await shell(); h.mount(); await settle(); h.ctx.setActiveTab('history');
  h.ctx.setApiKeyDraft(defaultSettings.providerId, 'synthetic-input');
  h.presence({ [providerOptions[2].id]: true });
  h.notify({ type: 'credentials_updated' }); await settle();
  expect(h.ctx.setupRequired()).toBe(false);
  h.presence({}); h.notify({ type: 'settings_changed' }); await settle();
  expect(h.ctx.setupRequired()).toBe(true);
  expect(h.actions.tab()).toBe('history');
  h.ctx.setActiveTab('setup');
  expect(h.ctx.apiKeyDrafts()[defaultSettings.providerId]).toBe('synthetic-input');
});

test('failed and stale presence reads cannot falsely open setup; a later successful read can', async () => {
  const h = await shell(); h.fail(true); h.mount(); await settle();
  expect(h.actions.tab()).toBe('history');
  expect(h.ctx.setupRequired()).toBe(false);
  h.fail(false);
  let release!: (value: Record<string, boolean>) => void;
  h.hold(new Promise((resolve) => { release = resolve; }));
  const stale = h.ctx.refreshProviderKeyPresence();
  h.hold(); h.presence({ [providerOptions[1].id]: true });
  await h.ctx.refreshProviderKeyPresence(); release({}); await stale;
  expect(h.actions.tab()).toBe('history');
  expect(h.ctx.setupRequired()).toBe(false);
  const empty = await shell(); empty.fail(true); empty.mount(); await settle();
  empty.fail(false); await empty.ctx.refreshProviderKeyPresence();
  expect(empty.actions.tab()).toBe('setup');
});

let render: (tab: string, step?: number, required?: boolean) => string;
beforeAll(async () => {
  const fixture = await compileRenderer(new URL('./helpers/DashboardFixture.svelte', import.meta.url));
  render = (tab, step = 1, required = true) => fixture({ initialState: {
    tab, setup: { step }, providerPresenceKnown: true,
    providerKeyPresence: required ? {} : { [defaultSettings.providerId]: true }
  } });
}, 60_000);

test('setup renders the shared controls in two ordered steps with the current static lit indicator', () => {
  const first = render('setup');
  expect(first).toContain('Welcome to Echo');
  for (const permission of ['Microphone', 'Accessibility', 'Input Monitoring']) expect(first).toContain(permission);
  expect(first).toMatch(/<button(?![^>]*disabled)[^>]*>Continue<\/button>/);
  expect(first).not.toContain('Check key');
  expect(first).toMatch(/<li(?=[^>]*class="[^"]*echo-lit)(?=[^>]*aria-current="step")[^>]*>1[\s\S]*Permissions/);
  const second = render('setup', 2);
  expect(second).toContain('Check key'); expect(second).toContain('Paste your Groq key');
  expect(second).toMatch(/<button[^>]*>(?:<!--[^>]*-->)*<svg[^>]*aria-hidden="true"[^>]*>(?:(?!<\/svg>)[\s\S])*<\/svg>Find saved key<\/button>/);
  expect(first).not.toContain('Find saved key');
  expect(second).toContain('Start using Echo'); expect(second).not.toContain('Desktop permissions');
  expect(second).toMatch(/<li(?=[^>]*class="[^"]*echo-lit)(?=[^>]*aria-current="step")[^>]*>2[\s\S]*Provider and key/);
});

test('setup leaves every rail page inactive while Settings section targets remain active', () => {
  const nav = (tab: string) => render(tab).match(/<nav\b[\s\S]*?<\/nav>/)![0];
  expect(nav('setup')).not.toContain('aria-current="page"');
  expect(nav('setup')).not.toContain('echo-lit');
  expect(nav('permissions')).toMatch(/<button(?=[^>]*aria-label="Settings")(?=[^>]*aria-current="page")(?=[^>]*class="[^"]*echo-lit)/);
});

test('History shows a quiet setup entry only when needed; Settings offers one entry with the matching label', () => {
  expect(render('history')).toContain('Finish setup');
  expect(render('history', 1, false)).not.toContain('Finish setup');
  for (const required of [true, false]) {
    const settings = render('settings', 1, required);
    expect(settings.match(/<button[^>]*class="setup-entry[^>]*>/g)).toHaveLength(1);
    expect(settings).toContain(required ? 'Finish setup to start transcribing' : 'Run setup again');
    expect(settings).not.toContain(required ? 'Run setup again' : 'Finish setup');
  }
});

// Keep: mounting and refreshing every provider/marker combination never saves or probes.
for (const provider of providerOptions) test(`${provider.label} setup refresh uses metadata without automatic credential work`, async () => {
  for (const saved of [false, true]) {
    const h = await shell(saved ? provider.id : undefined);
    h.mount(); await settle();
    await h.ctx.updateSettings({ providerKeyStatuses: { [provider.id]: 'valid' } });
    await settle();
    await h.ctx.refreshProviderKeyPresence();
    expect(h.ctx.setupRequired()).toBe(!saved);
    expect(h.calls).not.toContain('store');
    expect(h.calls).not.toContain('probe');
  }
});

// Keep: the dictation alert points to both recovery paths without changing its label.
test('a missing provider key alert names Find saved key under the Provider setup label', () => {
  const state = initialDashboardState();
  const controller = createDashboardController(state, { tick: async () => {}, focusPermissions() {} });
  state.snapshot = { ...state.snapshot, state: 'error', lastError: 'missing_provider_key' };
  expect(controller.alert()).toEqual({ label: 'Provider setup',
    message: 'Provider key missing. Add a key or use Find saved key in Settings > Provider.' });
});
