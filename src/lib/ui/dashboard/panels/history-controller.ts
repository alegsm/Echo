import type { DashboardContext } from '../context';
import type { TranscriptRecord } from '#lib/core/transcript.ts';

export type RetentionPreview = { days: number | undefined; deleteCount: number; label: string; retainCount: number };
export function initialHistoryState(initial: { historyLoading?: boolean; historyError?: string | null; historyLoadError?: string | null; totalCount?: number } = {}) {
  return { copiedId: null as string | null, historyLoading: initial.historyLoading ?? false,
    historyError: initial.historyError ?? null, historyLoadError: initial.historyLoadError ?? null,
    totalCount: initial.totalCount ?? 0, outcome: '', pendingDelete: null as TranscriptRecord | null,
    clearOpen: false, retentionPreview: null as RetentionPreview | null };
}
export type HistoryState = ReturnType<typeof initialHistoryState>;
type HistoryContext = Pick<DashboardContext, 'runtime' | 'history' | 'loadHistory' | 'settings' | 'updateSettings' | 'setActionError'>;
type HistoryEffects = {
  tick: () => Promise<void>;
  deleteButtons: () => Pick<HTMLElement, 'focus'>[];
  focusHeading: () => void;
  retentionInputs: () => { value: string; checked: boolean }[];
};

export function dayLabel(value: string) {
  return new Date(value).toLocaleDateString(undefined, { year: 'numeric', month: 'long', day: 'numeric' });
}
export function timeLabel(value: string) {
  return new Date(value).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
}

export function createHistoryController(ctx: HistoryContext, state: HistoryState, effects: HistoryEffects) {
  let copyTimer: ReturnType<typeof setTimeout> | undefined;
  function dispose() { clearTimeout(copyTimer); }
  function mount() {
    const runtime = ctx.runtime();
    if (!runtime) return;
    const unsubscribe = runtime.subscribe((event) => {
      if (event.type === 'history_changed') void loadHistory();
    });
    void loadHistory();
    return unsubscribe;
  }
  async function loadHistory(limit?: number) {
    state.historyLoadError = null;
    try {
      await ctx.loadHistory(limit);
    } catch {
      state.historyLoadError = 'History could not be loaded. Try again.';
    }
  }

  async function refreshTotalCount() {
    state.totalCount = await ctx.runtime()?.getHistoryCount().catch(() => ctx.history().length) ?? ctx.history().length;
  }

  async function loadMore() {
    state.historyLoading = true;
    try {
      await loadHistory(ctx.history().length + 100);
    } finally {
      state.historyLoading = false;
    }
  }

  function markCopied(id: string) {
    state.copiedId = id;
    if (copyTimer) clearTimeout(copyTimer);
    copyTimer = setTimeout(() => {
      state.copiedId = null;
    }, 1800);
  }

  async function copyTranscript(record: TranscriptRecord | undefined) {
    const runtime = ctx.runtime();
    if (!record || !runtime) return;
    ctx.setActionError(null);
    state.historyError = null;

    try {
      await runtime.copyTranscript(record.id);
      markCopied(record.id);
    } catch {
      state.historyError = "Couldn't copy";
    }
  }

  async function deleteTranscript() {
    const record = state.pendingDelete;
    const runtime = ctx.runtime();
    if (!record || !runtime) return;
    const priorIndex = ctx.history().findIndex((item) => item.id === record.id);
    state.pendingDelete = null;
    try {
      const affected = await runtime.deleteTranscript(record.id);
      state.outcome = affected ? 'Transcript deleted.' : 'Transcript was already removed.';
      await restoreListFocus(priorIndex);
    } catch (error) {
      await loadHistory();
      state.historyError = error instanceof Error ? error.message : 'Transcript could not be deleted.';
      state.outcome = 'History changed in another window. The list was refreshed.';
      effects.focusHeading();
    }
  }

  async function clearHistory() {
    const runtime = ctx.runtime();
    if (!runtime) return;
    state.clearOpen = false;
    try {
      const affected = await runtime.clearHistory();
      state.outcome = `${affected} transcript${affected === 1 ? '' : 's'} deleted.`;
    } catch (error) {
      await loadHistory();
      state.historyError = error instanceof Error ? error.message : 'History could not be cleared.';
      state.outcome = 'History changed in another window. The list was refreshed.';
    }
    effects.focusHeading();
  }

  async function chooseRetention(label: string) {
    const runtime = ctx.runtime();
    if (!runtime) return;
    const days = label === 'Forever' ? undefined : Number.parseInt(label, 10);
    const current = ctx.settings().historyRetentionDays;
    const isReduction = days !== undefined && (current === undefined || days < current);
    if (!isReduction) return applyRetention(days);
    state.historyError = null;
    try {
      const preview = await runtime.previewHistoryRetention(days);
      if (preview.deleteCount === 0) return applyRetention(days);
      state.retentionPreview = { days, label, ...preview };
    } catch {
      state.historyError = 'Retention preview could not be loaded. Try again.';
    }
  }

  async function applyRetention(days: number | undefined) {
    state.retentionPreview = null;
    try {
      await ctx.updateSettings({ historyRetentionDays: days });
      state.outcome = 'History retention updated.';
    } catch (error) {
      await loadHistory();
      state.historyError = error instanceof Error ? error.message : 'Retention could not be updated.';
      state.outcome = 'Settings changed in another window. Current values were reloaded.';
    }
  }

  async function restoreListFocus(priorIndex: number) {
    await effects.tick();
    const buttons = effects.deleteButtons();
    buttons[Math.min(Math.max(0, priorIndex), buttons.length - 1)]?.focus();
    if (buttons.length === 0) effects.focusHeading();
  }

  function restoreRetentionSelection() {
    for (const input of effects.retentionInputs()) {
      input.checked = input.value === String(ctx.settings().historyRetentionDays ?? 'forever');
    }
  }

  async function selectRetention(label: string) {
    restoreRetentionSelection();
    try {
      await chooseRetention(label);
    } finally {
      await effects.tick();
      restoreRetentionSelection();
    }
  }

  function retentionLabel() {
    const days = ctx.settings().historyRetentionDays;
    return days === undefined ? 'Forever' : `${days} days`;
  }
  return { mount, dispose, loadHistory, refreshTotalCount, loadMore, copyTranscript, deleteTranscript, clearHistory,
    applyRetention, selectRetention, retentionLabel };
}
