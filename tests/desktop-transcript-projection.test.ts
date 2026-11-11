import { expect, test } from 'bun:test';
import { createOverlayController, initialOverlayState, overlayPresentation } from '../src/lib/ui/overlay/overlay-controller';
import type { EchoSnapshot } from '../src/lib/core/types';

// Keep: completion follows authoritative snapshot identity through the production controller.
function announce(snapshotState: EchoSnapshot['state'], currentId: string | undefined, completedId: string) {
  const state = initialOverlayState();
  const transcript = (id: string) => ({ id }) as NonNullable<EchoSnapshot['lastTranscript']>;
  state.snapshot = { ...state.snapshot, state: snapshotState, lastTranscript: currentId ? transcript(currentId) : undefined };
  state.actionError = 'existing error';
  state.copyStatus = 'Copied';
  let announcements = 0;
  const controller = createOverlayController(state, {
    reducedMotion: true, refreshPreviewWindow() {}, clearTerminalStateTimer() {},
    showTerminalStateTemporarily() { announcements++; state.terminalStateVisible = true; }
  });
  controller.handleRuntimeEvent({ type: 'transcript_completed', transcript: transcript(completedId) });
  controller.dispose();
  return { ...state, announcements };
}

test.each([
  ['recording', 'prior', 'prior', 0],
  ['done', 'newer', 'older', 0],
  ['done', undefined, 'older', 0],
  ['done', 'current', 'current', 1],
] as const)('completion announcement respects authoritative %s snapshot and transcript identity', (state, currentId, completedId, expected) => {
  const result = announce(state, currentId, completedId);
  expect(result.announcements).toBe(expected);
  expect(result.snapshot.state).toBe(state);
  expect(result.snapshot.lastTranscript?.id).toBe(currentId);
  expect(result.actionError).toBe(expected ? null : 'existing error');
  expect(result.copyStatus).toBeNull();
  expect(overlayPresentation(result).statusTone).toBe(expected ? 'success' : state === 'recording' ? 'active' : 'idle');
});
