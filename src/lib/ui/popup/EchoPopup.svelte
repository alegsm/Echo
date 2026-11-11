<script lang="ts">
  import type { Snippet } from 'svelte';
  import { createPopupController } from './popup-controller';

  let {
    open,
    onClose,
    title,
    pretitle,
    subtitle,
    showClose = true,
    children,
    footer,
  }: {
    open: boolean;
    onClose: () => void;
    title: string;
    pretitle?: string;
    subtitle?: string;
    showClose?: boolean;
    children: Snippet;
    footer?: Snippet;
  } = $props();
  const popupId = $props.id();
  const titleId = `${popupId}-title`;
  let dialog = $state<HTMLDialogElement>();
  const { mount, handleCancel, handleKeydown } = createPopupController({
    dialog: () => dialog, activeElement: () => document.activeElement as HTMLElement | null,
    onClose: () => onClose()
  });
  $effect(() => {
    if (!open || !dialog) return;
    return mount();
  });
</script>

<dialog bind:this={dialog} aria-modal="true" aria-labelledby={titleId} class="echo-popup-overlay"
  oncancel={handleCancel} onkeydown={handleKeydown} onclick={(event) => { if (event.target === event.currentTarget) onClose(); }} tabindex="-1">
  <div class="echo-popup">
    <div class="echo-popup__container modal-container">
      <div class="echo-popup__header modal-header">
        <div class="echo-popup__header-titles">
          {#if pretitle}
            <span class="echo-popup__pretitle">{pretitle}</span>
          {/if}
          <h2 id={titleId} class="echo-popup__title">{title}</h2>
          {#if subtitle}
            <p class="echo-popup__subtitle">{subtitle}</p>
          {/if}
        </div>

        {#if showClose}
          <div class="echo-popup__close">
            <button type="button" aria-label="Close" onclick={onClose}>×</button>
          </div>
        {/if}
      </div>

      <div class="echo-popup__body modal-body">
        {@render children()}
      </div>

      {#if footer}
        <div class="echo-popup__footer modal-footer">
          {@render footer()}
        </div>
      {/if}
    </div>
  </div>
</dialog>

<style>
  .echo-popup-overlay { position: fixed; inset: 0; width: 100%; height: 100%; max-width: none; max-height: none; margin: 0; padding: 0; border: 0; background: transparent; color: var(--echo-fg); }
  .echo-popup-overlay[open] { display: flex; align-items: center; justify-content: center; }
  .echo-popup-overlay::backdrop { background: rgba(0, 0, 0, 0.92); -webkit-backdrop-filter: blur(4px); backdrop-filter: blur(4px); }
  .echo-popup {
    display: flex;
    flex-direction: column;
    width: 480px;
    max-width: 90vw;
    font-family: var(--font, 'Inter', ui-sans-serif, system-ui, sans-serif);
  }

  .echo-popup__container {
    display: flex;
    flex-direction: column;
    width: 100%;
    overflow: hidden;
    position: relative;
    border: 1px solid var(--echo-hairline, rgba(255, 255, 255, 0.07));
    border-radius: var(--r-card, 20px);
    background: rgb(10, 13, 19);
    min-height: 0;
  }

  .echo-popup__header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    padding: 20px 24px 16px;
    border-bottom: 1px solid var(--echo-divider, rgba(255, 255, 255, 0.06));
  }

  .echo-popup__header-titles {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
    flex: 1 1 auto;
  }

  .echo-popup__pretitle {
    font-family: inherit;
    font-size: 10px;
    font-weight: 400;
    text-transform: uppercase;
    letter-spacing: 0.12em;
    color: var(--echo-fg-3, rgba(236, 232, 226, 0.6));
    line-height: 1;
  }

  .echo-popup__title {
    font-family: inherit;
    font-size: 22px;
    font-weight: 500;
    letter-spacing: -0.015em;
    color: var(--echo-fg, #ece8e2);
    line-height: 1.2;
    margin: 0;
  }

  .echo-popup__subtitle {
    font-family: inherit;
    font-size: 14px;
    font-weight: 400;
    line-height: 1.45;
    color: var(--echo-fg-2, rgba(236, 232, 226, 0.74));
    margin: 4px 0 0;
    max-width: 64ch;
  }

  .echo-popup__close {
    flex-shrink: 0;
    margin-left: 16px;
  }

  .echo-popup__body {
    padding: 8px 24px 20px;
    overflow-y: auto;
    min-height: 0;
    font-family: inherit;
    font-size: 15px;
    line-height: 1.55;
    color: var(--echo-fg-2, rgba(236, 232, 226, 0.74));
    display: flex;
    flex-direction: column;
    gap: 16px;
    flex: 1 1 auto;
  }

  .echo-popup__footer {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 12px;
    min-height: 60px;
    padding: 0 24px;
    border-top: 1px solid var(--echo-divider, rgba(255, 255, 255, 0.06));
    border-radius: 0;
    background: var(--echo-well, rgba(0, 0, 0, 0.24));
  }

  .echo-popup__close button {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--echo-fg-2, rgba(236, 232, 226, 0.74));
    font: inherit;
    font-size: 24px;
    cursor: pointer;
  }

  .echo-popup__close button:hover {
    background: var(--echo-fill-2, rgba(255, 255, 255, 0.08));
    color: var(--echo-fg, #ece8e2);
  }

  .echo-popup__close button:focus-visible {
    outline: 2px solid var(--tone-record, #bdeeff);
    outline-offset: 2px;
  }

  @media (prefers-reduced-motion: reduce) {
    .echo-popup,
    .echo-popup__container {
      transition: none !important;
      animation: none !important;
    }
  }
</style>
