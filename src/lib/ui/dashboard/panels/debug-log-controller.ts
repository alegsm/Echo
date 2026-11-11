import type { DashboardContext } from '../context';
import type { DiagnosticStatus } from '#lib/runtime/debug-logging.ts';

export type DebugLogState = {
  diagnosticStatus: DiagnosticStatus;
  logPath: string | undefined;
  logPathState: 'loading' | 'ready' | 'unavailable';
  actionMessage: string;
  isSaving: boolean;
};
type DebugLogEffects = {
  loadContext: () => Promise<{ logFilePath: string }>;
  subscribeStatus: (listener: (status: DiagnosticStatus) => void) => () => void;
  attach: () => Promise<unknown>;
  detach: () => Promise<void>;
};

export function createDebugLogController(ctx: Pick<DashboardContext, 'updateSettings'>, state: DebugLogState, effects: DebugLogEffects) {
  async function saveDiagnosticsSettings(patch: { debugLoggingEnabled: boolean }) {
    if (state.isSaving) return;
    state.isSaving = true;
    state.actionMessage = '';
    try {
      await ctx.updateSettings(patch);
      if (patch.debugLoggingEnabled) await effects.attach();
      else await effects.detach();
    } catch {
      state.actionMessage = 'Could not change detailed logging. Try again.';
    } finally {
      state.isSaving = false;
    }
  }

  async function loadDiagnosticContext() {
    state.logPathState = 'loading';
    try {
      const context = await effects.loadContext();
      state.logPath = context.logFilePath;
      state.logPathState = 'ready';
    } catch {
      state.logPath = undefined;
      state.logPathState = 'unavailable';
    }
  }

  return { saveDiagnosticsSettings, loadDiagnosticContext,
    mount() {
      void loadDiagnosticContext();
      return effects.subscribeStatus((next) => { state.diagnosticStatus = next; });
    }
  };
}
