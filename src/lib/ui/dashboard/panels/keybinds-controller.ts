import type { EchoSettings } from '#lib/core/settings.ts';
import type { DashboardContext } from '../context';

export type KeybindsState = { capturingHotkey: boolean; savingSettings: boolean };
type KeybindsDom = {
  contains: (target: EventTarget | null) => boolean;
  isEditable: (target: EventTarget | null) => boolean;
  isChangeButton: (target: EventTarget | null) => boolean;
};
type KeybindsContext = Pick<DashboardContext, 'settings' | 'hotkeyDraft' | 'setHotkeyDraft' | 'setActionError' | 'updateSettings'>;

export function createKeybindsController(ctx: KeybindsContext, state: KeybindsState, dom: KeybindsDom, platform = typeof navigator === 'undefined' ? '' : navigator.platform) {
  function shortcutKey(event: KeyboardEvent) {
    const modifierKeys = new Set(['Meta', 'Control', 'Alt', 'Shift', 'Fn']);
    if (modifierKeys.has(event.key)) return undefined;

    if (/^Key[A-Z]$/.test(event.code)) return event.code.slice(3);
    if (/^Digit[0-9]$/.test(event.code)) return event.code.slice(5);
    if (/^F([1-9]|1[0-9]|2[0-4])$/.test(event.code)) return event.code;

    const namedKeys: Record<string, string> = {
      Space: 'Space',
      Tab: 'Tab',
      Enter: 'Enter',
      Escape: 'Escape',
      ArrowUp: 'ArrowUp',
      ArrowDown: 'ArrowDown',
      ArrowLeft: 'ArrowLeft',
      ArrowRight: 'ArrowRight',
      Backquote: 'Backquote',
      Minus: 'Minus',
      Equal: 'Equal',
      BracketLeft: 'BracketLeft',
      BracketRight: 'BracketRight',
      Backslash: 'Backslash',
      Semicolon: 'Semicolon',
      Quote: 'Quote',
      Comma: 'Comma',
      Period: 'Period',
      Slash: 'Slash'
    };

    return namedKeys[event.code];
  }

  function shortcutFromKeyboardEvent(event: KeyboardEvent) {
    if (event.key === 'Fn' || event.code === 'Fn') return 'Fn';
    if (
      event.ctrlKey &&
      event.metaKey &&
      !event.altKey &&
      !event.shiftKey &&
      (event.key === 'Meta' ||
        event.key === 'Control' ||
        event.code === 'MetaLeft' ||
        event.code === 'MetaRight' ||
        event.code === 'ControlLeft' ||
        event.code === 'ControlRight')
    ) {
      return 'Control+Windows';
    }

    const key = shortcutKey(event);
    if (!key) return undefined;

    const modifiers: string[] = [];
    if (event.metaKey) modifiers.push(platform.toLowerCase().includes('win') ? 'Super' : 'CommandOrControl');
    if (event.ctrlKey) modifiers.push('Control');
    if (event.altKey) modifiers.push('Alt');
    if (event.shiftKey) modifiers.push('Shift');

    return [...modifiers, key].join('+');
  }

  async function commitSettings(patch: Partial<EchoSettings>) {
    if (state.savingSettings) return;
    state.savingSettings = true;
    ctx.setActionError(null);
    try {
      await ctx.updateSettings(patch);
    } catch {
      ctx.setHotkeyDraft(ctx.settings().pushToTalkShortcut);
      ctx.setActionError('Settings could not be saved. Try again.');
    } finally {
      state.savingSettings = false;
    }
  }

  async function saveHotkey() {
    await commitSettings({ pushToTalkShortcut: ctx.hotkeyDraft().trim() });
  }

  function startHotkeyCapture() {
    ctx.setActionError(null);
    state.capturingHotkey = true;
    ctx.setHotkeyDraft('');
  }

  function cancelHotkeyCapture() {
    state.capturingHotkey = false;
    ctx.setHotkeyDraft(ctx.settings().pushToTalkShortcut);
  }

  function cancelHotkeyCaptureOutside(target: EventTarget | null) {
    if (state.capturingHotkey && !dom.contains(target)) {
      cancelHotkeyCapture();
    }
  }

  async function clearHotkey() {
    state.capturingHotkey = false;
    ctx.setHotkeyDraft('');
    await commitSettings({ pushToTalkShortcut: '' });
  }

  async function captureHotkey(event: KeyboardEvent) {
    if (!state.capturingHotkey || state.savingSettings) return;
    if (!dom.contains(event.target) && dom.isEditable(event.target)) {
      cancelHotkeyCapture();
      return;
    }
    if (event.key === 'Tab') {
      cancelHotkeyCapture();
      return;
    }
    if ((event.key === 'Enter' || event.key === ' ') && dom.isChangeButton(event.target)) return;

    event.preventDefault();
    event.stopPropagation();

    if (event.key === 'Escape') {
      cancelHotkeyCapture();
      return;
    }

    if (event.key === 'Backspace' || event.key === 'Delete') {
      await clearHotkey();
      return;
    }

    const shortcut = shortcutFromKeyboardEvent(event);
    if (!shortcut) return;

    ctx.setHotkeyDraft(shortcut);
    state.capturingHotkey = false;
    await saveHotkey();
  }

  async function savePasteAfterTranscription(on: boolean) {
    await commitSettings({ pasteAfterTranscription: on });
  }

  async function saveRestoreClipboard(on: boolean) {
    await commitSettings({ restoreClipboard: on });
  }

  return { startHotkeyCapture, cancelHotkeyCapture, cancelHotkeyCaptureOutside, captureHotkey,
    savePasteAfterTranscription, saveRestoreClipboard };
}
