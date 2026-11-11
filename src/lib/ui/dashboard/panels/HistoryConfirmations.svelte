<script lang="ts">
  import type { TranscriptRecord } from '#lib/core/transcript.ts';
  import EchoPopup from '#lib/ui/popup/EchoPopup.svelte';

  type RetentionPreview = {
    days: number | undefined;
    deleteCount: number;
    label: string;
    retainCount: number;
  };

  let {
    clearCount,
    clearOpen,
    deleteRecord,
    onApplyRetention,
    onClear,
    onCloseClear,
    onCloseDelete,
    onCloseRetention,
    onDelete,
    retentionPreview
  }: {
    clearCount: number;
    clearOpen: boolean;
    deleteRecord: TranscriptRecord | null;
    onApplyRetention(days: number | undefined): void;
    onClear(): void;
    onCloseClear(): void;
    onCloseDelete(): void;
    onCloseRetention(): void;
    onDelete(): void;
    retentionPreview: RetentionPreview | null;
  } = $props();
</script>

<EchoPopup
  open={deleteRecord !== null}
  onClose={onCloseDelete}
  pretitle="DELETE TRANSCRIPT"
  title="Delete this transcript?"
  subtitle="This removes the saved transcript from Echo history."
>
  <p class="confirm-copy">{deleteRecord?.text.slice(0, 180)}</p>
  {#snippet footer()}
    <span class="footer-status">1 TRANSCRIPT</span>
    <div class="footer-buttons">
      <button type="button" onclick={onCloseDelete}>Cancel</button>
      <button type="button" class="danger" onclick={onDelete}>Delete</button>
    </div>
  {/snippet}
</EchoPopup>

<EchoPopup
  open={clearOpen}
  onClose={onCloseClear}
  pretitle="CLEAR HISTORY"
  title="Delete all {clearCount} transcripts?"
  subtitle="This cannot be undone."
>
  <p class="confirm-copy">Echo will remove every saved transcript from native history.</p>
  {#snippet footer()}
    <span class="footer-status">{clearCount} SAVED</span>
    <div class="footer-buttons">
      <button type="button" onclick={onCloseClear}>Cancel</button>
      <button type="button" class="danger" onclick={onClear}>Clear history</button>
    </div>
  {/snippet}
</EchoPopup>

<EchoPopup
  open={retentionPreview !== null}
  onClose={onCloseRetention}
  pretitle="CHANGE RETENTION"
  title="Keep history for {retentionPreview?.label}?"
  subtitle="{retentionPreview?.deleteCount ?? 0} older transcripts will be deleted."
>
  <p class="confirm-copy">
    {retentionPreview?.retainCount ?? 0} transcripts will remain after this change.
  </p>
  {#snippet footer()}
    <span class="footer-status">RETENTION</span>
    <div class="footer-buttons">
      <button type="button" onclick={onCloseRetention}>Cancel</button>
      <button type="button" class="danger" onclick={() => onApplyRetention(retentionPreview?.days)}>Apply</button>
    </div>
  {/snippet}
</EchoPopup>

<style>
  button { min-height: 34px; padding: 0 16px; border: 1px solid var(--echo-hairline); border-radius: var(--r-bar); background: var(--echo-fill-1); color: var(--echo-fg); font: 500 13px/1 var(--font); cursor: pointer; }
  button:hover { background: var(--echo-fill-2); }
  button.danger { color: var(--tone-error-text); border-color: color-mix(in srgb, var(--tone-error) 30%, transparent); background: color-mix(in srgb, var(--tone-error) 10%, transparent); }
  button:focus-visible { outline: 2px solid var(--tone-record); outline-offset: 2px; }
  .footer-buttons {
    display: flex;
    gap: 8px;
  }

  .footer-status {
    color: var(--echo-fg-3);
    font: var(--t-body);
  }

  .confirm-copy {
    color: var(--echo-fg-2);
    margin: 0;
    overflow-wrap: anywhere;
  }
</style>
