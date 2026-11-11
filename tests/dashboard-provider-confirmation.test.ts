import { expect, test } from 'bun:test';
import type { HTMLButtonElement, HTMLDialogElement, HTMLInputElement, HTMLElement } from 'happy-dom';
import { clientHarness } from './helpers/mount-svelte';
import { defaultSettings } from '../src/lib/core/settings';

// Keep: intact ProviderPanel interactions through an injected native client; browser and OS acceptance remains separate.
for (const platform of ['macos', 'windows'] as const) {
  test(`${platform} explains storage before saving, traps focus and restores the cancelled draft`, async () => {
    const id = defaultSettings.providerId;
    const calls: string[] = [];
    let settings = { ...defaultSettings };
    const h = await clientHarness(`
      <script>
        import { provideDashboardContext } from './src/lib/ui/dashboard/context.js';
        import ProviderPanel from './src/lib/ui/dashboard/panels/ProviderPanel.svelte';
        let { context } = $props();
        provideDashboardContext(context);
      </script>
      <ProviderPanel />
    `);
    const drafts: Record<string, string> = { [id]: '' };
    let stored = false;
    const context = {
      settings: () => settings, snapshot: () => ({ state: 'idle' }), permissions: () => ({ platform }),
      apiKeyDrafts: () => drafts,
      providerKeyPresence: () => ({ [id]: stored }),
      setApiKeyDraft(key: string, draft: string) { drafts[key] = draft; }, setActionError() {},
      async updateSettings(patch: object) { calls.push('status'); settings = { ...settings, ...patch }; },
      async refreshProviderKeyPresence() { calls.push('refresh'); },
      runtime: () => ({
        async setProviderApiKey() { calls.push('save'); stored = true; },
        async probeProviderKey() { calls.push('probe'); }, async deleteProviderApiKey() { calls.push('delete'); }
      })
    };
    const dialogs = h.window.HTMLDialogElement.prototype;
    dialogs.showModal = function () { this.setAttribute('open', '');  };
    dialogs.close = function () { this.removeAttribute('open'); this.dispatchEvent(new h.window.Event('close')); };
    const settle = async () => { for (let n = 0; n < 16; n++) { await Promise.resolve(); h.flush(); } };
    const button = (text: string, root: HTMLElement = h.target) => [...root.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.trim() === text)!;
    try {
      h.mount({ context });
      const key = h.target.querySelector<HTMLInputElement>('#provider-key')!;
      key.value = 'synthetic-input'; key.dispatchEvent(new h.window.Event('input', { bubbles: true })); h.flush();
      button('Check key').click(); await settle();
      const dialog = h.target.querySelector<HTMLDialogElement>('dialog[open]')!;
      expect(dialog).toBeTruthy();
      expect(calls).toEqual([]);
      const heading = dialog.querySelector('h3')!;
      expect(dialog.getAttribute('aria-labelledby')).toBe(heading.id);
      expect(heading.textContent).toBe('Save Groq key');
      expect(dialog.textContent).toContain(`Echo will save your key in ${platform === 'macos' ? 'macOS Keychain' : 'Windows Credential Manager'} on this device.`);
      expect(dialog.textContent).toContain('Echo reads it only to call Groq');
      expect(dialog.textContent?.includes('macOS may ask you to allow Echo access')).toBe(platform === 'macos');
      const cancel = button('Cancel', dialog);
      const save = button(platform === 'macos' ? 'Save to Keychain' : 'Save to Credential Manager', dialog);
      expect(h.window.document.activeElement).toBe(cancel);
      for (const [shiftKey, expected] of [[true, save], [false, cancel], [false, save], [false, cancel]] as const) {
        dialog.dispatchEvent(new h.window.KeyboardEvent('keydown', { key: 'Tab', shiftKey, bubbles: true, cancelable: true }));
        expect(h.window.document.activeElement).toBe(expected);
      }
      dialog.dispatchEvent(new h.window.Event('cancel', { cancelable: true })); await settle();
      expect(calls).toEqual([]);
      expect(key.value).toBe('synthetic-input');
      expect(h.window.document.activeElement).toBe(key);
      button('Check key').click(); await settle();
      button('Cancel', dialog).click(); await settle();
      expect(calls).toEqual([]);
      expect(key.value).toBe('synthetic-input');
      button('Check key').click(); await settle();
      save.click(); save.click(); await settle();
      expect(calls.filter((call) => ['save', 'status', 'probe'].includes(call)).slice(0, 3)).toEqual(['save', 'status', 'probe']);
      expect(calls.filter((call) => call === 'save')).toHaveLength(1);
      expect(drafts[id]).toBe('');
    } finally { await h.cleanup(); }
  });
}

// Keep: an existing saved key probes without opening consent or writing an empty draft.
test('an empty draft and saved key probes without a save dialog', async () => {
  const id = defaultSettings.providerId;
  const draft = '';
  const calls: string[] = [];
  let settings = { ...defaultSettings, providerKeySaved: { [id]: true } };
  const h = await clientHarness(`
    <script>
      import { provideDashboardContext } from './src/lib/ui/dashboard/context.js';
      import ProviderPanel from './src/lib/ui/dashboard/panels/ProviderPanel.svelte';
      let { context } = $props();
      provideDashboardContext(context);
    </script>
    <ProviderPanel />
  `);
  const context = {
    settings: () => settings, snapshot: () => ({ state: 'idle' }), permissions: () => ({ platform: 'macos' }),
    apiKeyDrafts: () => ({ [id]: draft }),
    providerKeyPresence: () => ({ [id]: true }),
    setApiKeyDraft() {}, setActionError() {},
    async updateSettings(patch: object) { settings = { ...settings, ...patch }; },
    async refreshProviderKeyPresence() {},
    runtime: () => ({
      async setProviderApiKey() { calls.push('save'); },
      async probeProviderKey() { calls.push('probe'); }, async deleteProviderApiKey() { calls.push('delete'); }
    })
  };
  h.window.HTMLDialogElement.prototype.showModal = function () { this.setAttribute('open', ''); };
  h.window.HTMLDialogElement.prototype.close = function () { this.removeAttribute('open'); };
  try {
    h.mount({ context });
    [...h.target.querySelectorAll<HTMLButtonElement>('button')].find((button) => button.textContent?.trim() === 'Check key')!.click();
    for (let n = 0; n < 16; n++) { await Promise.resolve(); h.flush(); }
    expect(h.target.querySelector('dialog[open]') !== null).toBe(false);
    expect(calls).toEqual(['probe']);
    expect(context.apiKeyDrafts()[id]).toBe(draft);
  } finally { await h.cleanup(); }
});
