import { createPhrasebookController, type PhrasebookState } from '../src/lib/ui/dashboard/panels/phrasebook-controller';
import { beforeAll, expect, test } from 'bun:test';
import { compileRenderer } from './helpers/render-svelte';
import { defaultSettings, providerOptions, type EchoSettings } from '../src/lib/core/settings';
import { createNativeStorageClient } from '../src/lib/runtime/desktop/native-storage-client';

let render: (terms?: string[], query?: string, providerId?: string) => string;
// Keep: intact component markup with the real dashboard context; native persistence needs host acceptance.
beforeAll(async () => {
  const fixture = await compileRenderer(new URL('./helpers/DashboardFixture.svelte', import.meta.url));
  render = (terms = [], query = '', providerId = defaultSettings.providerId) => fixture({
    component: 'phrasebook', initialState: { settings: { phrasebookTerms: terms, providerId } },
    phrasebook: { searchQuery: query }
  });
}, 60_000);

test('separate Add and list cards show counts, Search, named removal and case-insensitive A to Z rows', () => {
  const html = render(['zebra', 'Beta', 'alpha']);
  expect(html).toMatch(/<h1[^>]*>Phrasebook<\/h1>/);
  expect(html).toContain('3 terms, A to Z');
  expect(html).toContain('Add a term, for example a name or product');
  expect(html).toMatch(/aria-label="Search terms"/);
  expect(html.match(/class="dashboard-card /g)).toHaveLength(2);
  expect(html.indexOf('>alpha<')).toBeLessThan(html.indexOf('>Beta<'));
  expect(html.indexOf('>Beta<')).toBeLessThan(html.indexOf('>zebra<'));
  expect(html).toContain('aria-label="Remove Beta"');
  expect(html).not.toMatch(/Provider hint support|PROVIDER SUPPORT/);
  expect(render()).toContain('0 terms, A to Z');
});

test('search filters immediately, highlights literal matches and escapes saved text', () => {
  const html = render(['Beta', '<img src=x onerror=alert(1)>'], 'IMG');
  expect(html).toMatch(/<mark[^>]*>img<\/mark>/);
  expect(html).toContain('&lt;');
  expect(html).not.toContain('<img');
  expect(html).not.toContain('Remove Beta');
  expect(render(['Beta'], 'absent')).toContain('No terms match');
});

test('one saved term uses singular counts and the fields have decorative icons', () => {
  const html = render(['Saved']);
  expect(html).toMatch(/>1 term<\/p>/);
  expect(html).toContain('1 term, A to Z');
  expect(html).toMatch(/aria-hidden="true"[^>]*>\+<\/span>/);
  expect(html).toMatch(/<svg[^>]*aria-hidden="true"/);
});

test('Unicode case-insensitive matching highlights the original substring', () => {
  expect(render(['İstanbul'], 'stan')).toMatch(/<mark[^>]*>stan<\/mark>/);
  expect(render(['A+B'], '+')).toMatch(/<mark[^>]*>\+<\/mark>/);
});

for (const provider of providerOptions) test(`${provider.label} keeps Add, search and removal available without a support notice`, () => {
  const html = render(['Saved'], '', provider.id);
  const controls = html.match(/<(?:input|button)\b[^>]*>/g) ?? [];
  expect(controls).toHaveLength(4);
  for (const control of controls) expect(/\bdisabled\b/.test(control)).toBe(false);
  expect(html).toContain('>Saved<');
  expect(html.includes(`${provider.label} ignores phrasebook terms. Your terms are kept.`)).toBe(false);
  expect(render(['Saved'], 'absent', provider.id)).not.toContain('>Saved<');
});

async function handlers(terms: string[] = []) {
  let settings: EchoSettings = { ...defaultSettings, phrasebookTerms: terms }, draft = '', fail = false;
  const calls: Array<{ command: string; args: any }> = [], focus: string[] = [];
  const storage = createNativeStorageClient({ async invoke(command: string, args: any) {
    calls.push({ command, args });
    if (fail) throw new Error('private native detail');
    settings = { ...settings, ...args.patch };
    return { current: { settings, revision: calls.length }, previous: { settings, revision: 0 }, changedFields: ['phrasebookTerms'] } as any;
  }} as any);
  const ctx = { settings: () => settings, phrasebookInput: () => draft,
    setPhrasebookInput: (value: string) => { draft = value; },
    updateSettings: async (patch: Partial<EchoSettings>) => (await storage.updateSettings(calls.length, patch)).current.settings };
  const state: PhrasebookState = { searchQuery: '', feedback: '', saving: false };
  const controller = createPhrasebookController(ctx, state, {
    tick: async () => {}, focusInput: () => { focus.push('add'); }, removeButtons: () => []
  });
  const actions = { ...controller, feedback: () => state.feedback, search: (query: string) => { state.searchQuery = query; } };
  return { actions, calls, focus, ctx, draft: () => draft, fail: () => { fail = true; },
    provider: (providerId: EchoSettings['providerId']) => { settings = { ...settings, providerId }; } };
}

test('successful Add returns focus to the field for another term', async () => {
  const h = await handlers();
  h.ctx.setPhrasebookInput('New');
  await h.actions.addPhrasebookTerm();
  expect(h.focus).toEqual(['add']);
});

test.each([
  { draft: '   ', terms: [], message: /enter/i },
  { draft: ' ALPHA ', terms: ['alpha'], message: /already/i },
  { draft: 'x'.repeat(129), terms: [], message: /128/ },
  { draft: 'é'.repeat(65), terms: [], message: /128/ },
  { draft: 'extra', terms: Array.from({ length: 100 }, (_, i) => `term${i}`), message: /100/ }
])('invalid Add saves nothing and explains why: $message', async ({ draft, terms, message }) => {
  const h = await handlers([...terms]);
  h.ctx.setPhrasebookInput(draft);
  await h.actions.addPhrasebookTerm();
  expect(h.calls).toEqual([]);
  expect(h.actions.feedback()).toMatch(message);
});

test('add, search and remove use the native settings client and preserve unrelated settings', async () => {
  const h = await handlers(['zebra']);
  h.ctx.setPhrasebookInput(' Alpha ');
  await h.actions.addPhrasebookTerm();
  expect(h.calls[0]).toMatchObject({ command: 'update_persisted_settings', args: { patch: { phrasebookTerms: ['zebra', 'Alpha'] } } });
  expect(h.draft()).toBe('');
  expect(h.actions.filteredTerms()).toEqual(['Alpha', 'zebra']);
  h.actions.search('AL');
  expect(h.actions.filteredTerms()).toEqual(['Alpha']);
  await h.actions.removePhrasebookTerm('Alpha');
  expect(h.ctx.settings().phrasebookTerms).toEqual(['zebra']);
  expect(h.ctx.settings().providerId).toBe(defaultSettings.providerId);
  expect(h.focus).toEqual(['add', 'add']);
  h.ctx.setPhrasebookInput('Another');
  await h.actions.addPhrasebookTerm();
  expect(h.calls).toHaveLength(3);
});

// Keep: every provider can use the same native phrasebook path, after provider selection.
for (const provider of providerOptions) test(`${provider.label} adds, searches and removes saved terms`, async () => {
  const h = await handlers(['Saved']);
  h.provider(provider.id);
  h.ctx.setPhrasebookInput('New');
  await h.actions.addPhrasebookTerm();
  expect(h.ctx.settings().phrasebookTerms).toEqual(['Saved', 'New']);
  h.actions.search('NEW');
  expect(h.actions.filteredTerms()).toEqual(['New']);
  await h.actions.removePhrasebookTerm('New');
  expect(h.calls).toHaveLength(2);
  expect(h.ctx.settings().phrasebookTerms).toEqual(['Saved']);
  expect(h.ctx.settings().providerId).toBe(provider.id);
});

test('failed saves retain the draft and saved terms and report a safe retry message', async () => {
  const h = await handlers(['Saved']);
  h.fail();
  h.ctx.setPhrasebookInput('New');
  await h.actions.addPhrasebookTerm();
  expect(h.draft()).toBe('New');
  expect(h.actions.feedback()).toMatch(/try again/i);
  await h.actions.removePhrasebookTerm('Saved');
  expect(h.ctx.settings().phrasebookTerms).toEqual(['Saved']);
  expect(h.actions.feedback()).toMatch(/try again/i);
  expect(h.actions.feedback()).not.toContain('private');
});
