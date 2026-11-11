import { defaultSettings } from '#lib/core/settings.ts';
import type { EchoErrorCode, EchoRuntimeEvent, EchoSnapshot } from '#lib/core/types.ts';
import { emptyShell, reduceShell } from './overlay-shell-phase';
import { emptyPreview, PREVIEW_FADE_MS, reduceOverlayPreview } from './overlay-preview';

export const errorLabels: Record<EchoErrorCode, string> = {
  shortcut_conflict: 'Shortcut unavailable',
  microphone_permission_denied: 'Microphone denied',
  microphone_unavailable: 'Microphone unavailable',
  capture_failed: 'Capture failed',
  empty_audio: 'No speech detected',
  missing_provider_key: 'Provider key missing',
  provider_timeout: 'Provider timed out',
  provider_failed: 'Provider failed',
  clipboard_failed: 'Clipboard failed',
  insertion_failed: 'Paste failed',
  history_write_failed: 'History write failed',
  not_implemented: 'Unavailable'
};

export function initialOverlayState() {
  return {
    snapshot: { mode: 'desktop', state: 'idle', activeProviderId: 'groq-whisper-turbo', activeShortcut: defaultSettings.pushToTalkShortcut } as EchoSnapshot,
    handsFree: false, actionError: null as string | null, terminalStateVisible: false,
    copyStatus: null as string | null, preview: emptyPreview, shell: emptyShell,
    recordingProgress: null as Extract<EchoRuntimeEvent, { type: 'recording_progress' }> | null,
    warningAnnouncement: '',
    captureIntentId: undefined as string | undefined,
    transcriptionNotice: ''
  };
}
export type OverlayState = ReturnType<typeof initialOverlayState>;
type OverlayEffects = {
  reducedMotion: boolean | (() => boolean);
  refreshPreviewWindow: () => void;
  clearTerminalStateTimer: () => void;
  showTerminalStateTemporarily: () => void;
};

export function overlayPresentation(state: OverlayState) {
  const { snapshot, handsFree, actionError, terminalStateVisible } = state;
  let warningText = '';
  if (state.recordingProgress) {
    const remainingSeconds = (state.recordingProgress.limitMs - state.recordingProgress.elapsedMs) / 1000;
    if (remainingSeconds <= 60) {
      const countdown = Math.max(0, Math.ceil(remainingSeconds) - 1);
      warningText = `Recording stops in ${Math.floor(countdown / 60)}:${String(countdown % 60).padStart(2, '0')}`;
    }
  }
  const isBusy = snapshot.state === 'requesting_microphone' || snapshot.state === 'recording';
  const isLoading = snapshot.state === 'stopping' || snapshot.state === 'transcribing' || snapshot.state === 'inserting';
  const hasVisibleError = terminalStateVisible && Boolean(actionError || snapshot.lastError || snapshot.state === 'error');
  const hasVisibleSuccess = terminalStateVisible && snapshot.state === 'done';
  const recordingTone = snapshot.state === 'recording' && handsFree ? 'handsFree' : 'active';
  const statusTone = hasVisibleError ? 'error'
    : hasVisibleSuccess ? 'success'
    : isLoading ? 'loading'
    : warningText ? 'warning'
    : isBusy ? recordingTone
    : 'idle';
  const activityText = snapshot.state === 'recording' ? 'Listening'
    : snapshot.state === 'transcribing' ? 'Transcribing'
    : snapshot.state === 'stopping' ? 'Finalizing'
    : snapshot.state === 'inserting' ? 'Pasting'
    : snapshot.state === 'requesting_microphone' ? 'Requesting microphone'
    : 'Working';
  return { isBusy, isLoading, hasVisibleError, hasVisibleSuccess, statusTone, activityText, warningText } as const;
}

export function createOverlayController(state: OverlayState, effects: OverlayEffects) {
  const reducedMotion = () => typeof effects.reducedMotion === 'function' ? effects.reducedMotion() : effects.reducedMotion;
  let disposed = false;
  let finishPreviewFade: (() => void) | undefined;
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;
  let notice: { kind: 'gap' | 'retrying'; ordinal?: number } | undefined;
  function showNotice(kind: 'gap' | 'retrying', ordinal: number | undefined) {
    notice = { kind, ordinal };
    state.transcriptionNotice = kind === 'gap' ? 'Transcription gap. Other speech is kept.' : 'Retrying a transcription piece…';
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(hideNotice, 4_000);
  }
  function hideNotice() {
    clearTimeout(noticeTimer);
    noticeTimer = undefined;
    notice = undefined;
    state.transcriptionNotice = '';
    effects.refreshPreviewWindow();
  }
  let previewFadeTimer: ReturnType<typeof setTimeout> | undefined;
  function handleRuntimeEvent(event: EchoRuntimeEvent) {
    if (disposed) return;
    const hadWarning = Boolean(overlayPresentation(state).warningText);
    if (event.type === 'partial_transcript_changed') {
      const projection = event.projection;
      if (projection.phase === 'active' && projection.ordinal === -1 &&
          projection.dictationId !== state.captureIntentId) {
        state.captureIntentId = projection.dictationId;
        state.recordingProgress = null;
        state.warningAnnouncement = '';
        clearTimeout(noticeTimer);
        notice = undefined;
        state.transcriptionNotice = '';
      }
      if (projection.dictationId === state.captureIntentId && projection.phase === 'active' && projection.status) {
        // A gap notice always runs its full time; a recovery clears only its own piece's retry notice.
        const ordinal = projection.statusOrdinal;
        if (projection.status === 'gap') showNotice('gap', ordinal);
        else if (projection.status === 'retrying' && notice?.kind !== 'gap') showNotice('retrying', ordinal);
        else if (projection.status === 'recovered' && notice?.kind === 'retrying' && notice.ordinal === ordinal) hideNotice();
        effects.refreshPreviewWindow();
      }
    }
    if (event.type === 'recording_progress' && event.intentId === state.captureIntentId &&
        (state.snapshot.state === 'recording' || state.snapshot.state === 'requesting_microphone')) {
      // Capture-begin projections own intent identity; native progress owns the clock.
      const previous = state.recordingProgress;
      if (!previous || (event.sessionId === previous.sessionId && event.elapsedMs >= previous.elapsedMs)) {
        state.recordingProgress = event;
        if (!state.warningAnnouncement && overlayPresentation(state).warningText) {
          state.warningAnnouncement = 'Recording stops in one minute.';
        }
      }
    }
    // Intent resets before the native stop completes. Keep amber until capture exits.
    if (event.type === 'hands_free_changed' && (event.active || state.snapshot.state !== 'recording')) {
      state.handsFree = event.active;
    }
    if (event.type === 'state_changed' && event.snapshot.state !== 'recording' &&
        event.snapshot.state !== 'requesting_microphone') {
      state.handsFree = false;
      if (!['stopping', 'transcribing'].includes(event.snapshot.state)) {
        clearTimeout(noticeTimer);
        notice = undefined;
        state.transcriptionNotice = '';
      }
      state.recordingProgress = null;
      state.warningAnnouncement = '';
      state.shell = reduceShell(state.shell, { type: 'released', reducedMotion: reducedMotion() });
    }
    const nextPreview = reduceOverlayPreview(state.preview, event);
    if (nextPreview !== state.preview) {
      state.preview = nextPreview;
      clearTimeout(previewFadeTimer);
      finishPreviewFade = undefined;
      if (nextPreview.phase === 'fading' && nextPreview.dictationId) {
        const dictationId = nextPreview.dictationId;
        const generation = nextPreview.generation;
        const finishFade = () => {
          if (disposed || state.preview.dictationId !== dictationId || state.preview.generation !== generation) return;
          state.preview = reduceOverlayPreview(state.preview, { type: 'fade_finished', dictationId, generation });
          finishPreviewFade = undefined;
          previewFadeTimer = undefined;
          effects.refreshPreviewWindow();
        };
        finishPreviewFade = finishFade;
        if (reducedMotion()) finishFade();
        else previewFadeTimer = setTimeout(finishFade, PREVIEW_FADE_MS);
      }
      effects.refreshPreviewWindow();
    }
    if (event.type === 'state_changed') {
      state.snapshot = event.snapshot;
      if (isRuntimeBusyState(event.snapshot.state)) {
        state.actionError = null;
        state.copyStatus = null;
        state.terminalStateVisible = false;
        effects.clearTerminalStateTimer();
      }
    }

    if (event.type === 'transcript_completed') {
      state.copyStatus = null;
      if (state.snapshot.state === 'done' && state.snapshot.lastTranscript?.id === event.transcript.id) {
        state.actionError = null;
        effects.showTerminalStateTemporarily();
      }
    }

    if (event.type === 'error') {
      state.actionError = event.message || errorLabels[event.code];
      state.copyStatus = null;
      effects.showTerminalStateTemporarily();
    }
    if (hadWarning !== Boolean(overlayPresentation(state).warningText)) effects.refreshPreviewWindow();
  }

  function isRuntimeBusyState(state: EchoSnapshot['state']) {
    return (
      state === 'requesting_microphone' ||
      state === 'recording' ||
      state === 'stopping' ||
      state === 'transcribing' ||
      state === 'inserting'
    );
  }

  return {
    handleRuntimeEvent,
    motionChanged() {
      if (disposed || !reducedMotion()) return;
      clearTimeout(previewFadeTimer);
      finishPreviewFade?.();
    },
    dispose() { disposed = true; finishPreviewFade = undefined; clearTimeout(previewFadeTimer); clearTimeout(noticeTimer); }
  };
}
