<script lang="ts">
  import { getDashboardContext } from '../context.js';
  import { createKeybindsController, type KeybindsState } from './keybinds-controller';
  import DashboardCard from '../layout/DashboardCard.svelte';

  const ctx = getDashboardContext();
  const viewState = $state<KeybindsState>({ capturingHotkey: false, savingSettings: false });
  let shortcutCard = $state<HTMLElement | null>(null);
  const degraded = $derived(ctx.settings().pushToTalkShortcut === 'Control+Windows' &&
    ctx.snapshot().nativeModifierAvailable === false);
  const fallbackShortcut = $derived(ctx.settings().fallbackShortcut?.replace(/CommandOrControl|Control/g, 'Ctrl').replace(/Super/g, 'Windows'));
  let currentShortcut = $derived(ctx.settings().pushToTalkShortcut || 'None');
  const { startHotkeyCapture, cancelHotkeyCapture, cancelHotkeyCaptureOutside, captureHotkey,
    savePasteAfterTranscription, saveRestoreClipboard } = createKeybindsController(ctx, viewState, {
    contains: (target) => target instanceof Node && !!shortcutCard?.contains(target),
    isEditable: (target) => target instanceof Element && !!target.closest('input, textarea, select, [contenteditable]:not([contenteditable="false"])'),
    isChangeButton: (target) => target instanceof Element && !!target.closest('.change-shortcut')
  });
</script>

<svelte:window
  onkeydown={(event) => void captureHotkey(event)}
  onpointerdown={(event) => cancelHotkeyCaptureOutside(event.target)}
/>

<section aria-labelledby="settings-shortcut" bind:this={shortcutCard} onfocusout={(event) => event.relatedTarget instanceof Element && cancelHotkeyCaptureOutside(event.relatedTarget)}>
  <DashboardCard padding="var(--card-pad)">
    <h2 id="settings-shortcut" tabindex="-1">Shortcut</h2>
    <div class="shortcut-row">
      <kbd class="echo-keycap" aria-live="polite">{viewState.capturingHotkey ? 'Press a shortcut…' : currentShortcut}</kbd>
      <button type="button" class="change-shortcut" disabled={viewState.savingSettings} onclick={viewState.capturingHotkey ? cancelHotkeyCapture : startHotkeyCapture}>{viewState.capturingHotkey ? 'Cancel' : 'Change'}</button>
    </div>
    <div class="shortcut-help">
      {#if degraded}
        <p role="status">Native shortcut unavailable. {#if ctx.settings().fallbackShortcut}Fallback: {fallbackShortcut}. Press once to record, then press again to transcribe.{:else}Choose another shortcut to record.{/if}</p>
      {:else}
        <p>Hold this key to record and release to transcribe. Tap once to record hands-free, then tap again to transcribe.</p>
      {/if}
    </div>
  </DashboardCard>
</section>

<section aria-labelledby="paste-heading">
  <DashboardCard padding="var(--card-pad)">
    <h2 id="paste-heading">Paste</h2>
    <div class="paste-row">
      <div><span>Paste after transcription</span><p>Off: copied only.</p></div>
      <button type="button" class="echo-switch" role="switch" aria-checked={ctx.settings().pasteAfterTranscription} aria-label="Paste after transcription" disabled={viewState.savingSettings} onclick={() => void savePasteAfterTranscription(!ctx.settings().pasteAfterTranscription)}></button>
    </div>
    <div class="paste-row">
      <div><span>Restore clipboard after paste</span><p>Puts back what you had copied.</p></div>
      <button type="button" class="echo-switch" role="switch" aria-checked={ctx.settings().restoreClipboard} aria-label="Restore clipboard after paste" disabled={viewState.savingSettings} onclick={() => void saveRestoreClipboard(!ctx.settings().restoreClipboard)}></button>
    </div>
  </DashboardCard>
</section>

<style>
  section { min-width: 0; display: flex; }
  h2 { margin: 0 0 12px; font: var(--t-caps); letter-spacing: 0.08em; text-transform: uppercase; color: var(--echo-fg-3); }
  .shortcut-row { display: flex; align-items: center; flex-wrap: wrap; gap: 14px; margin: 6px 0 12px; }
  .change-shortcut { height: 34px; padding: 0 16px; border: 0; border-radius: var(--r-bar); background: var(--echo-fill-1); box-shadow: inset 0 0 0 1px var(--echo-hairline); color: var(--echo-fg); font: 500 13px/1 var(--font); cursor: pointer; }
  .change-shortcut:hover { background: var(--echo-fill-2); }
  .change-shortcut:disabled { opacity: 0.45; cursor: default; }
  .shortcut-help p { margin: 0; font: var(--t-meta); color: var(--echo-fg-3); }
  .paste-row { display: flex; align-items: center; justify-content: space-between; gap: 16px; padding: 13px 0; }
  .paste-row + .paste-row { border-top: 1px solid var(--echo-divider); }
  .paste-row span { font: var(--t-label); color: var(--echo-fg); }
  .paste-row p { margin: 3px 0 0; font: var(--t-meta); color: var(--echo-fg-3); }
</style>
