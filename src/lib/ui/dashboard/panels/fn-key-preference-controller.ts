import type { createFnKeyPreferenceClient, FnKeyAction, FnKeyPreference } from '#lib/runtime/desktop/fn-key-preference-client.ts';

export type FnKeyState = { preference: FnKeyPreference | undefined; pending: boolean; message: string; alertRevision?: number };
type FnKeyEffects = {
  tick: () => Promise<void>;
  activeElement: () => Element | null | undefined;
  focusRecheck: () => void;
  openSettings: () => Promise<unknown>;
  isDesktop: () => boolean;
};

export function createFnKeyPreferenceController(client: ReturnType<typeof createFnKeyPreferenceClient>, state: FnKeyState, effects: FnKeyEffects) {
  function recognized(current: FnKeyPreference['current']) {
    return current === null || (typeof current === 'number' && [0, 1, 2, 3].includes(current));
  }

  function settingName(current: FnKeyPreference['current']) {
    if (current === null) return 'System default';
    if (current === 'unrecognized') return 'Unrecognized value';
    return ['Do Nothing', 'Change Input Source', 'Show Emoji & Symbols', 'Start Dictation'][current] ?? 'Unrecognized value';
  }

  async function refresh() {
    if (!effects.isDesktop() || state.pending) return;
    state.pending = true;
    try {
      state.preference = await client.read();
      state.message = '';
    } catch {
      state.message = 'Fn key setting could not be checked. Try again.';
      state.alertRevision = (state.alertRevision ?? 0) + 1;
    } finally {
      state.pending = false;
    }
  }

  async function changePreference(action: FnKeyAction, trigger?: HTMLButtonElement) {
    if (state.pending || !state.preference?.supported || (action === 'use_fn' && !recognized(state.preference.current))) return;
    const hadFocus = trigger && effects.activeElement() === trigger;
    state.pending = true;
    state.message = '';
    try {
      state.preference = await client.change(action);
      await effects.tick();
      if (hadFocus && (!trigger.isConnected || trigger.disabled)) effects.focusRecheck();
    } catch {
      state.message = 'Fn key setting could not be changed or checked. Re-check and try again.';
    } finally {
      state.pending = false;
    }
  }

  async function openSettings() {
    try {
      await effects.openSettings();
    } catch {
      state.message = 'Could not open System Settings. Try again.';
      state.alertRevision = (state.alertRevision ?? 0) + 1;
    }
  }

  return { recognized, settingName, refresh, changePreference, openSettings,
    mount() { void refresh(); }
  };
}
