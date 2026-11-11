<script lang="ts">
  import { getDashboardContext } from '../context.js';
  import { createTranscriptionController, type TranscriptionState } from './transcription-controller';
  import DashboardCard from '../layout/DashboardCard.svelte';

  const ctx = getDashboardContext();
  const languages = [
    ['en', 'English'], ['es', 'Spanish'], ['pt', 'Portuguese'], ['fr', 'French'],
    ['de', 'German'], ['it', 'Italian'], ['nl', 'Dutch'], ['ja', 'Japanese'],
    ['ko', 'Korean'], ['zh', 'Chinese']
  ];
  let savedLanguage = $derived(ctx.settings().language ?? '');
  let unlistedLanguage = $derived(savedLanguage && !languages.some(([code]) => code === savedLanguage));
  const viewState = $state<TranscriptionState>({ promptDraft: undefined, savingLanguage: false, savingPrompt: false, languageError: null, promptError: null });
  const { currentPrompt, saveLanguage, editPrompt, savePrompt } = createTranscriptionController(ctx, viewState);
</script>

<section aria-labelledby="settings-transcription">
  <DashboardCard padding="var(--card-pad)">
    <div class="card-head"><h2 id="settings-transcription" tabindex="-1">Transcription</h2></div>
    <div class="language-row">
      <div><label for="transcription-language">Language</label><p id="language-help">Auto-detect identifies the spoken language; a fixed language is faster and steadier for short dictations.</p></div>
      <select id="transcription-language" value={savedLanguage} disabled={viewState.savingLanguage} aria-invalid={!!viewState.languageError} aria-describedby={viewState.languageError ? 'language-help language-error' : 'language-help'} onchange={saveLanguage}>
        <option value="">Auto-detect</option>
        {#each languages as [code, name]}<option value={code}>{name}</option>{/each}
        {#if unlistedLanguage}<option value={savedLanguage}>{savedLanguage}</option>{/if}
      </select>
    </div>
    {#if viewState.languageError}<p class="error" id="language-error" role="alert">{viewState.languageError}</p>{/if}
    <div class="prompt-row">
      <div><label for="transcription-prompt">Custom prompt</label><p id="prompt-help">Style or context sent with every recording.</p></div>
      <textarea id="transcription-prompt" rows="3" value={currentPrompt()} readonly={viewState.savingPrompt} aria-busy={viewState.savingPrompt} aria-invalid={!!viewState.promptError} aria-describedby={viewState.promptError ? 'prompt-help prompt-error' : 'prompt-help'} oninput={editPrompt} onblur={savePrompt}></textarea>
      {#if viewState.promptError}<p class="error" id="prompt-error" role="alert">{viewState.promptError}</p>{/if}
    </div>
  </DashboardCard>
</section>

<style>
  section { grid-column: 1 / -1; min-width: 0; }
  .card-head { display: flex; align-items: center; min-height: 28px; margin-bottom: 8px; }
  h2 { margin: 0; font: var(--t-caps); letter-spacing: 0.08em; text-transform: uppercase; color: var(--echo-fg-3); }
  .language-row { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 18px; align-items: center; padding: 12px 0; }
  .prompt-row { display: grid; gap: 10px; padding-top: 12px; border-top: 1px solid var(--echo-divider); }
  label { font: var(--t-label); color: var(--echo-fg); }
  p { margin: 3px 0 0; font: var(--t-meta); color: var(--echo-fg-3); }
  select, textarea { box-sizing: border-box; min-width: 0; border: 0; border-radius: var(--r-control); background: var(--echo-well); box-shadow: inset 0 0 0 1px var(--echo-hairline); color: var(--echo-fg); font: var(--t-body); transition: box-shadow var(--fast) ease; }
  select { height: 38px; max-width: 100%; padding: 0 14px; appearance: none; -webkit-appearance: none; padding-right: 32px; background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='6' fill='none' stroke='%23ece8e2' stroke-opacity='.7' stroke-width='1.4'%3E%3Cpath d='M1 1l4 4 4-4'/%3E%3C/svg%3E"); background-repeat: no-repeat; background-position: right 13px center; cursor: pointer; }
  option { background: var(--echo-canvas); }
  textarea { width: 100%; padding: 11px 14px; line-height: 1.5; resize: vertical; }
  select:disabled { opacity: 0.45; }
  .error { color: var(--tone-error-text); overflow-wrap: anywhere; }
  @media (max-width: 520px) { .language-row { grid-template-columns: minmax(0, 1fr); gap: 10px; } }
</style>
