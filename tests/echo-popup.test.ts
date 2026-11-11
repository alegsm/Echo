import { expect, test } from 'bun:test';
import { compileRenderer, sourcePath } from './helpers/render-svelte';
import { createPopupController } from '../src/lib/ui/popup/popup-controller';

// Keep: owned dialog lifecycle and keyboard boundary; real browser semantics remain host acceptance.
function popupActions() {
  const controls = [0, 1].map(() => ({ focus() { doc.activeElement = this; } }));
  let closed = 0;
  const trigger = { isConnected: true, focus() { doc.activeElement = this; } };
  const doc = { activeElement: trigger as unknown };
  const dialog = { open: false, showModal() { this.open = true; controls[0].focus(); }, close() { this.open = false; }, querySelectorAll: () => controls, focus() { doc.activeElement = this; } };
  const actions = createPopupController({ dialog: () => dialog as unknown as HTMLDialogElement,
    activeElement: () => doc.activeElement as HTMLElement, onClose: () => { closed++; } });
  return { actions, controls, dialog, doc, trigger, closed: () => closed };
}
test('Echo dialog opens modally, returns focus and closes on Escape', () => {
  const h = popupActions();
  const cleanup = h.actions.mount()!;
  expect(h.dialog.open).toBe(true);
  expect(h.doc.activeElement).toBe(h.controls[0]);
  let prevented = false;
  h.actions.handleCancel({ preventDefault() { prevented = true; } });
  expect(prevented).toBe(true);
  expect(h.closed()).toBe(1);
  cleanup();
  expect(h.dialog.open).toBe(false);
  expect(h.doc.activeElement).toBe(h.trigger);
});
test.each([false, true])('Echo dialog traps Tab with shift=%s at its keyboard boundary', (shiftKey) => {
  const h = popupActions();
  h.doc.activeElement = h.controls[shiftKey ? 0 : 1];
  let prevented = false;
  h.actions.handleKeydown({ key: 'Tab', shiftKey, preventDefault() { prevented = true; } });
  expect(prevented).toBe(true);
  expect(h.doc.activeElement).toBe(h.controls[shiftKey ? 1 : 0]);
});
test('Echo dialog takes its accessible name from the popup title and renders its footer', async () => {
  const render = await compileRenderer(`<script>import Popup from '${sourcePath('src/lib/ui/popup/EchoPopup.svelte')}';</script>
    <Popup open={true} title="Delete transcript?" onClose={() => {}}>
      <p>Saved transcript</p>
      {#snippet footer()}<button>Delete</button>{/snippet}
    </Popup>`);
  const html = render();
  const titleId = html.match(/<dialog[^>]*aria-labelledby="([^"]+)"/)?.[1];
  expect(titleId).toBeDefined();
  const heading = html.match(/<h2\b[^>]*>Delete transcript\?<\/h2>/)?.[0];
  expect(heading).toContain(`id="${titleId}"`);
  expect(html).toContain('Saved transcript');
  expect(html).toContain('<button>Delete</button>');
});

// Keep: disconnected triggers receive no focus and empty dialogs retain the keyboard boundary.
test('Echo dialog handles a removed trigger and an empty focus list', () => {
  const h = popupActions();
  const cleanup = h.actions.mount()!;
  h.trigger.isConnected = false;
  h.controls.length = 0;
  let prevented = 0;
  h.actions.handleKeydown({ key: 'Tab', shiftKey: false, preventDefault() { prevented++; } });
  expect(h.doc.activeElement).toBe(h.dialog);
  expect(prevented).toBe(1);
  h.actions.handleKeydown({ key: 'a', shiftKey: false, preventDefault() { prevented++; } });
  expect(prevented).toBe(1);
  cleanup();
  expect(h.doc.activeElement).toBe(h.dialog);
});
