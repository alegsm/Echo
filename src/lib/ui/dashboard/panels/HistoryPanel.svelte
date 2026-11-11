<script module lang="ts">
  export type HistoryInitialState = {
    historyLoading?: boolean;
    historyError?: string | null;
    historyLoadError?: string | null;
    totalCount?: number;
  };
</script>

<script lang="ts">
  import { dashboardGlow } from '../dashboard-motion';
  import { onDestroy, tick, untrack } from 'svelte';
  import { createHistoryController, initialHistoryState, dayLabel, timeLabel } from './history-controller';
  import './history-panel.css';
  import { getDashboardContext } from '../context.js';
  import type { TranscriptRecord } from '#lib/core/transcript.ts';
  import AlertBanner from '../AlertBanner.svelte';
  import HistoryConfirmations from './HistoryConfirmations.svelte';
  import PageHeader from '../layout/PageHeader.svelte';
  import DashboardCard from '../layout/DashboardCard.svelte';

  let { initialState = {} }: { initialState?: HistoryInitialState } = $props();
  const initial = untrack(() => initialState);
  const ctx = getDashboardContext();

  const viewState = $state(initialHistoryState(initial));
  let records = $derived(ctx.history());
  let count = $derived(viewState.totalCount || records.length);
  const retentionChoices = [7, 30, 90, undefined];
  let retention = $derived(ctx.settings().historyRetentionDays);
  let latest = $derived(records[0]);
  let days = $derived([...new Set(records.slice(1).map((record) => dayLabel(record.createdAt)))]);

  const { mount, dispose, loadHistory, refreshTotalCount, loadMore, copyTranscript, deleteTranscript, clearHistory,
    selectRetention, applyRetention, retentionLabel } = createHistoryController(ctx, viewState, {
    tick, deleteButtons: () => [...document.querySelectorAll<HTMLButtonElement>('[data-history-delete]')],
    focusHeading: () => document.querySelector<HTMLElement>('.history-panel h1')?.focus(),
    retentionInputs: () => [...document.querySelectorAll<HTMLInputElement>('input[name="history-retention"]')]
  });
  onDestroy(dispose);
  $effect(() => { records.length; void refreshTotalCount(); });
  $effect(mount);
</script>

<svelte:window onfocus={() => void loadHistory()} />

<div class="history-panel">
  <PageHeader
    title="History"
    subtitle={`${count} transcript${count === 1 ? '' : 's'} · ${retention === undefined ? 'kept forever' : `kept for ${retentionLabel()}`}`}
  >
    {#snippet actions()}
      {#if !retentionChoices.includes(retention)}
        <span class="retention-legacy">Current: {retentionLabel()}</span>
      {/if}
      <div class="retention-control" role="radiogroup" aria-label="Keep transcripts for">
        {#each retentionChoices as choice}
          <label>
            <input type="radio" name="history-retention" value={choice ?? 'forever'} checked={retention === choice}
              aria-label={choice === undefined ? 'Keep forever' : `Keep ${choice} days`}
              onchange={() => void selectRetention(choice === undefined ? 'Forever' : `${choice} days`)} />
            <span {@attach dashboardGlow}>{choice === undefined ? 'Forever' : `${choice}d`}</span>
          </label>
        {/each}
      </div>
      <button type="button" class="history-clear"
        disabled={viewState.historyLoading || count === 0}
        onclick={() => (viewState.clearOpen = true)}
      >
        Clear all
      </button>
    {/snippet}
  </PageHeader>

  <div class="history-panel__body">
    {#if ctx.setupRequired()}
      <button type="button" class="setup-entry" onclick={() => ctx.setActiveTab('setup')}>Finish setup to start transcribing</button>
    {/if}
    <span class="sr-only" role="status" aria-live="polite">{viewState.outcome}</span>
    {#if viewState.historyError || viewState.historyLoadError}
      <AlertBanner label="History" message={viewState.historyError ?? viewState.historyLoadError ?? ''} />
    {/if}

    {#if count === 0}
      <DashboardCard>
        <div class="empty-state-wrap"><span class="empty-state-message">Nothing here yet. Hold your shortcut and speak.</span></div>
      </DashboardCard>
    {:else}
      {#if latest}
        <article {@attach dashboardGlow} class="history-latest echo-lit" aria-labelledby="latest-heading">
          <header class="history-row-header">
            <h2 id="latest-heading">Latest · {dayLabel(latest.createdAt)} {timeLabel(latest.createdAt)}</h2>
            {@render transcriptActions(latest, true)}
          </header>
          <p class="transcript">{latest.text}</p>
        </article>
      {/if}
      {#each days as day}
        <section class="history-day" data-history-day={day} aria-label={day}>
          <h2>{day}</h2>
          <ol class="history-list">
            {#each records.slice(1).filter((record) => dayLabel(record.createdAt) === day) as record (record.id)}
              <li class="history-row">
                <time datetime={record.createdAt}>{timeLabel(record.createdAt)}</time>
                <p class="transcript">{record.text}</p>
                {@render transcriptActions(record)}
              </li>
            {/each}
          </ol>
        </section>
      {/each}
      {#if records.length < count}
        <div class="load-more-row">
          <button type="button" class="history-load-more" disabled={viewState.historyLoading} onclick={() => void loadMore()}>
            {viewState.historyLoading ? 'Loading' : `Load more (${records.length} of ${count})`}
          </button>
        </div>
      {/if}
    {/if}
  </div>
</div>

{#snippet transcriptActions(record: TranscriptRecord, labelled = false)}
  <div class="history-row-actions" class:labelled>
    <button type="button" aria-label={`${viewState.copiedId === record.id ? 'Copied' : 'Copy'} transcript from ${dayLabel(record.createdAt)} ${timeLabel(record.createdAt)}`} title={viewState.copiedId === record.id ? 'Copied' : 'Copy'} onclick={() => copyTranscript(record)}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
        {#if viewState.copiedId === record.id}<path d="m5 12 4 4L19 6" />
        {:else}<rect x="8" y="8" width="12" height="12" rx="2" /><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3" />{/if}
      </svg>
      {#if labelled}<span>{viewState.copiedId === record.id ? 'Copied' : 'Copy'}</span>{/if}
    </button>
    <button type="button" class="danger" aria-label={`Delete transcript from ${dayLabel(record.createdAt)} ${timeLabel(record.createdAt)}`} title="Delete" data-history-delete={record.id} onclick={() => (viewState.pendingDelete = record)}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7M14 10v7" /></svg>
      {#if labelled}<span>Delete</span>{/if}
    </button>
  </div>
{/snippet}

<HistoryConfirmations
  clearCount={count}
  clearOpen={viewState.clearOpen}
  deleteRecord={viewState.pendingDelete}
  retentionPreview={viewState.retentionPreview}
  onCloseDelete={() => (viewState.pendingDelete = null)}
  onCloseClear={() => (viewState.clearOpen = false)}
  onCloseRetention={() => (viewState.retentionPreview = null)}
  onDelete={() => void deleteTranscript()}
  onClear={() => void clearHistory()}
  onApplyRetention={(days) => void applyRetention(days)}
/>

<style>
  .setup-entry { align-self: flex-start; padding: 6px 4px; border: 0; background: transparent; color: var(--echo-fg-2); font: var(--t-meta); cursor: pointer; }
</style>
