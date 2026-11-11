type PopupEffects = {
  dialog: () => HTMLDialogElement | undefined;
  activeElement: () => HTMLElement | null;
  onClose: () => void;
};
const focusable = 'a[href], button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])';

export function createPopupController(effects: PopupEffects) {
  function mount() {
    const surface = effects.dialog();
    if (!surface) return;
    const trigger = effects.activeElement();
    surface.showModal();
    return () => {
      surface.close();
      if (trigger?.isConnected) trigger.focus();
    };
  }

  function handleCancel(event: Pick<Event, 'preventDefault'>) {
    event.preventDefault();
    effects.onClose();
  }

  function handleKeydown(event: Pick<KeyboardEvent, 'key' | 'shiftKey' | 'preventDefault'>) {
    const dialog = effects.dialog();
    if (event.key !== 'Tab' || !dialog) return;
    const controls = [...dialog.querySelectorAll<HTMLElement>(focusable)];
    const first = controls[0];
    const last = controls.at(-1);
    if (!first) { event.preventDefault(); dialog.focus(); return; }
    if (event.shiftKey && effects.activeElement() === first) {
      event.preventDefault(); last?.focus();
    } else if (!event.shiftKey && (effects.activeElement() === last || effects.activeElement() === dialog)) {
      event.preventDefault(); first.focus();
    }
  }
  return { mount, handleCancel, handleKeydown };
}
