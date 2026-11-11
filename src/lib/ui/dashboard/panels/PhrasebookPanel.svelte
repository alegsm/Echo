<script module lang="ts">
  export type PhrasebookInitialState = { searchQuery?: string };
</script>

<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { createPhrasebookController, type PhrasebookState } from './phrasebook-controller';
  import { getDashboardContext } from '../context.js';
  import PageHeader from '../layout/PageHeader.svelte';
  import DashboardCard from '../layout/DashboardCard.svelte';

  let { initialState = {} }: { initialState?: PhrasebookInitialState } = $props();
  const initial = untrack(() => initialState);
  const ctx = getDashboardContext();
  const viewState = $state<PhrasebookState>({ searchQuery: initial.searchQuery ?? '', feedback: '', saving: false });
  const { terms, query, termMatch, filteredTerms, addPhrasebookTerm, removePhrasebookTerm, handlePhrasebookInput } = createPhrasebookController(ctx, viewState, {
    tick, focusInput: () => document.querySelector<HTMLInputElement>('[name="phrasebook-term"]')?.focus(),
    removeButtons: () => [...document.querySelectorAll<HTMLButtonElement>('.phrasebook-panel .term-list button')]
  });
  let count = $derived(terms().length);
  let countLabel = $derived(`${count} ${count === 1 ? 'term' : 'terms'}`);
</script>

<div class="phrasebook-panel">
  <PageHeader title="Phrasebook" subtitle={countLabel} />
  <DashboardCard padding="var(--card-pad)">
    <form class="add-row" onsubmit={(event) => { event.preventDefault(); void addPhrasebookTerm(); }}>
      <div class="icon-field add-field">
        <span class="field-icon" aria-hidden="true">+</span>
        <input name="phrasebook-term" aria-label="Add a term" value={ctx.phrasebookInput()}
          placeholder="Add a term, for example a name or product" autocomplete="off" spellcheck={false}
          disabled={viewState.saving} aria-describedby={viewState.feedback ? 'phrasebook-feedback' : undefined}
          oninput={handlePhrasebookInput} />
      </div>
      <button class="add-button" type="submit" disabled={viewState.saving}>Add</button>
    </form>
    {#if viewState.feedback}<p class="feedback" id="phrasebook-feedback" role="alert">{viewState.feedback}</p>{/if}
  </DashboardCard>
  <DashboardCard padding="var(--card-pad)">
    <div class="list-head">
      <h2>{countLabel}, A to Z</h2>
      <div class="icon-field search-field">
        <svg class="field-icon" aria-hidden="true" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="8.5" cy="8.5" r="5.5" /><path d="m13 13 4 4" /></svg>
        <input name="phrasebook-search" type="search" aria-label="Search terms" placeholder="Search"
          value={viewState.searchQuery} oninput={(event) => { viewState.searchQuery = event.currentTarget.value; }} />
      </div>
    </div>
    <ul class="term-list" aria-label="Phrasebook terms">
      {#each filteredTerms() as term (term)}
        {@const match = termMatch(term)}
        <li class="term-row">
          <span class="term-text">{#if match}{term.slice(0, match.index)}<mark>{match[0]}</mark>{term.slice(match.index + match[0].length)}{:else}{term}{/if}</span>
          <button class="remove-button" type="button" aria-label={`Remove ${term}`} title={`Remove ${term}`}
            disabled={viewState.saving} onclick={() => void removePhrasebookTerm(term)}><span aria-hidden="true">×</span></button>
        </li>
      {/each}
    </ul>
    <p class="list-status" role="status">{#if !terms().length}No terms yet.{:else if !filteredTerms().length}No terms match “{viewState.searchQuery}”.{:else if query()}{filteredTerms().length} of {countLabel}{/if}</p>
  </DashboardCard>
</div>

<style>
  .phrasebook-panel { display: flex; flex-direction: column; gap: var(--card-gap); min-width: 0; }
  .add-row { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 10px; align-items: center; }
  .icon-field { position: relative; min-width: 0; }
  .icon-field input { padding-left: 38px; }
  .field-icon { position: absolute; left: 14px; top: 50%; transform: translateY(-50%); width: 16px; height: 16px; color: var(--echo-fg-3); pointer-events: none; }
  .add-field .field-icon { height: auto; font: var(--t-label); text-align: center; }
  .search-field { width: 240px; }
  input { box-sizing: border-box; width: 100%; min-width: 0; height: 46px; padding: 0 14px; border: 0; border-radius: var(--r-control); background: var(--echo-well); box-shadow: inset 0 0 0 1px var(--echo-hairline); color: var(--echo-fg); font: var(--t-body); }
  input::placeholder { color: var(--echo-fg-3); }
  .add-row input { font-size: 15px; }
  button { border: 0; cursor: pointer; font: var(--t-label); }
  .add-button { height: 34px; padding: 0 16px; border-radius: var(--r-bar); background: var(--echo-light); color: var(--echo-ink-on-light); font: 500 13px/1 var(--font); }
  .list-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 8px; }
  h2 { margin: 0; font: var(--t-caps); letter-spacing: 0.08em; text-transform: uppercase; color: var(--echo-fg-3); }
  .list-head input { height: 32px; }
  .term-list { list-style: none; margin: 0; padding: 0; }
  .term-row { box-sizing: border-box; display: flex; align-items: center; justify-content: space-between; gap: 12px; min-height: 44px; border-top: 1px solid var(--echo-divider); font: var(--t-label); color: var(--echo-fg); }
  .term-row:first-child { border-top: 0; }
  .term-text { min-width: 0; overflow-wrap: anywhere; }
  mark { background: none; color: var(--tone-record); }
  .remove-button { width: 30px; height: 30px; flex: none; display: grid; place-items: center; border-radius: var(--r-bar); background: transparent; color: var(--echo-fg-3); font-size: 20px; }
  .remove-button:hover:enabled { background: var(--echo-fill-2); color: var(--tone-error-text); }
  :disabled { opacity: 0.45; cursor: default; }
  .feedback, .list-status { margin: 0; font: var(--t-meta); color: var(--echo-fg-3); }
  .feedback { margin-top: 10px; color: var(--tone-error-text); }
  .list-status:empty { display: none; }
  @media (max-width: 520px) { .list-head { flex-wrap: wrap; } .search-field { width: 100%; } }
</style>
