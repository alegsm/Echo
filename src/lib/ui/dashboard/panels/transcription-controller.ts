import type { DashboardContext } from '../context';

export type TranscriptionState = {
  promptDraft: string | undefined;
  savingLanguage: boolean;
  savingPrompt: boolean;
  languageError: string | null;
  promptError: string | null;
};

export function createTranscriptionController(ctx: Pick<DashboardContext, 'settings' | 'updateSettings'>, state: TranscriptionState) {
  function currentPrompt() {
    return state.promptDraft ?? ctx.settings().customPrompt ?? '';
  }

  function validationError(error: unknown, field: string) {
    return (error instanceof Error ? error.message : error) === `invalid storage field: ${field}`;
  }

  async function saveLanguage(event: Event) {
    const select = event.currentTarget as HTMLSelectElement;
    state.languageError = null;
    state.savingLanguage = true;
    try {
      await ctx.updateSettings({ language: select.value || undefined });
    } catch (error) {
      state.languageError = validationError(error, 'language')
        ? 'Language must be a valid language tag, such as en or pt-BR.'
        : 'Language could not be saved. Try again.';
    } finally {
      select.value = ctx.settings().language ?? '';
      state.savingLanguage = false;
    }
  }

  function editPrompt(event: Event) {
    state.promptDraft = (event.currentTarget as HTMLTextAreaElement).value;
    state.promptError = null;
  }

  async function savePrompt() {
    const draft = currentPrompt();
    if (state.savingPrompt || draft === (ctx.settings().customPrompt ?? '')) return;
    state.promptError = null;
    state.savingPrompt = true;
    try {
      await ctx.updateSettings({ customPrompt: draft || undefined });
      if (state.promptDraft === draft) state.promptDraft = undefined;
    } catch (error) {
      state.promptError = validationError(error, 'customPrompt')
        ? 'Too long. Accented and non-Latin characters count extra.'
        : 'Custom prompt could not be saved. Try again.';
    } finally {
      state.savingPrompt = false;
    }
  }

  return { currentPrompt, saveLanguage, editPrompt, savePrompt };
}
