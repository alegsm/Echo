import type { DashboardContext } from '../context';

export type PhrasebookState = { searchQuery: string; feedback: string; saving: boolean };
type PhrasebookContext = Pick<DashboardContext, 'settings' | 'phrasebookInput' | 'setPhrasebookInput' | 'updateSettings'>;
type PhrasebookEffects = { tick: () => Promise<void>; focusInput: () => void; removeButtons: () => Pick<HTMLElement, 'focus'>[] };

export function createPhrasebookController(ctx: PhrasebookContext, state: PhrasebookState, effects: PhrasebookEffects) {
  function terms() { return ctx.settings().phrasebookTerms ?? []; }
  function query() { return state.searchQuery.trim(); }
  function termMatch(term: string) {
    if (!query()) return null;
    const literal = query().replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    return new RegExp(literal, 'iu').exec(term);
  }
  function filteredTerms() {
    return [...terms()].sort((a, b) => a.localeCompare(b, undefined, { sensitivity: 'base' }))
      .filter((term) => !query() || termMatch(term));
  }

  async function addPhrasebookTerm() {
    if (state.saving) return;
    const draft = ctx.phrasebookInput();
    const term = draft.trim();
    state.feedback = '';
    if (!term) { state.feedback = 'Enter a term.'; return; }
    if (terms().some((saved) => saved.toLowerCase() === term.toLowerCase())) {
      state.feedback = 'That term is already saved.'; return;
    }
    if (new TextEncoder().encode(term).length > 128) {
      state.feedback = 'Use 128 UTF-8 bytes or fewer. Accented characters count extra.'; return;
    }
    if (terms().length >= 100) { state.feedback = 'Keep up to 100 terms. Remove one first.'; return; }
    state.saving = true;
    try {
      await ctx.updateSettings({ phrasebookTerms: [...terms(), term] });
      if (ctx.phrasebookInput() === draft) ctx.setPhrasebookInput('');
    } catch {
      state.feedback = 'Term could not be saved. Try again.';
      return;
    } finally {
      state.saving = false;
    }
    await effects.tick();
    effects.focusInput();
  }

  async function removePhrasebookTerm(term: string) {
    if (state.saving) return;
    const removedIndex = filteredTerms().indexOf(term);
    state.feedback = '';
    state.saving = true;
    try {
      await ctx.updateSettings({ phrasebookTerms: terms().filter((saved) => saved !== term) });
    } catch {
      state.feedback = 'Term could not be removed. Try again.';
      return;
    } finally {
      state.saving = false;
    }
    await effects.tick();
    const buttons = effects.removeButtons();
    const next = buttons[Math.min(removedIndex, buttons.length - 1)];
    if (next) next.focus();
    else effects.focusInput();
  }

  function handlePhrasebookInput(event: Event) {
    ctx.setPhrasebookInput((event.currentTarget as HTMLInputElement).value);
    state.feedback = '';
  }
  return { terms, query, termMatch, filteredTerms, addPhrasebookTerm, removePhrasebookTerm, handlePhrasebookInput };
}
