import { expect, test } from 'bun:test';
import { compileRenderer } from './helpers/render-svelte';
import type { EchoRuntimeEvent, EchoSnapshot } from '../src/lib/core/types';
import { emptyShell } from '../src/lib/ui/overlay/overlay-shell-phase';
import { createOverlayController, initialOverlayState, overlayPresentation } from '../src/lib/ui/overlay/overlay-controller';

// Render the intact overlay surface. Native window placement and browser events
// remain host-harness proof; this checks the content for each runtime state.
test('expanded details omit timing numbers while retaining transcript Copy and recording Stop', async () => {
  const render = await compileRenderer(new URL('../src/lib/ui/overlay/OverlaySurface.svelte', import.meta.url));
  const model = {
    ...overlayPresentation(initialOverlayState()), expanded: true, hasVisibleSuccess: true, hasVisibleError: false,
    actionError: null, ariaAlertText: '', ariaStatusText: '', animationSpeed: 1, currentModelText: 'Synthetic model',
    errorLabel: '', haloPose: 'rest', particleCount: 60, previewWords: [], ringColors: [], shell: emptyShell,
    snapshot: { state: 'done', lastTranscript: { id: 'transcript-1', text: '<hello>', timings: { providerMs: 125 } } },
    timingRows: [['Provider', 125]], copyStatus: null,
  };
  const success = render({ model });
  expect(success).toContain('&lt;hello>');
  expect(success).not.toContain('<hello>');
  expect(success).toMatch(/<button[^>]*>Copy<\/button>/);
  const details = success.match(/<div class="detail"[^]*?<button[^>]*class="collapse-button"/)?.[0];
  expect(details).toBeDefined();
  expect(details).not.toMatch(/Timing metrics|Provider|125|\bms\b/);
  const recording = render({ model: { ...model, hasVisibleSuccess: false, isBusy: true, activityText: 'Listening', snapshot: { state: 'recording' } } });
  expect(recording).toContain('Listening');
  expect(recording).toMatch(/<button[^>]*>Stop<\/button>/);
});

// Keep: imported events and presentation use the same state as the component.
// Browser rendering and colour interpolation remain host-harness proof.
function createToneTrace() {
  const state = initialOverlayState();
  const controller = createOverlayController(state, {
    reducedMotion: true, refreshPreviewWindow() {}, clearTerminalStateTimer() {}, showTerminalStateTemporarily() {}
  });
  return (event: EchoRuntimeEvent) => {
    controller.handleRuntimeEvent(event);
    return overlayPresentation(state).statusTone;
  };
}

const stateEvent = (state: EchoSnapshot['state']): EchoRuntimeEvent => ({
  type: 'state_changed',
  snapshot: { mode: 'desktop', state, activeProviderId: 'groq-whisper-turbo', activeShortcut: 'Fn' }
});

test.each(['stopping', 'idle', 'error'] as const)('hands-free stays amber through reset until the snapshot exits recording to %s', (exit) => {
  const dispatch = createToneTrace();
  expect(dispatch(stateEvent('recording'))).toBe('active');
  expect(dispatch({ type: 'hands_free_changed', active: true })).toBe('handsFree');
  const trace = [
    dispatch({ type: 'hands_free_changed', active: false }),
    dispatch(stateEvent('recording')),
    dispatch(stateEvent(exit)),
  ];
  expect(trace).toEqual(['handsFree', 'handsFree', exit === 'stopping' ? 'loading' : 'idle']);
  expect(dispatch(stateEvent('requesting_microphone'))).toBe('active');
  expect(dispatch(stateEvent('recording'))).toBe('active');
});

test('a hands-free startup latch survives repeated startup snapshots, colours only recording and clears on capture exit', () => {
  const dispatch = createToneTrace();
  expect(dispatch(stateEvent('requesting_microphone'))).toBe('active');
  expect(dispatch({ type: 'hands_free_changed', active: true })).toBe('active');
  expect(dispatch(stateEvent('requesting_microphone'))).toBe('active');
  expect(dispatch(stateEvent('recording'))).toBe('handsFree');
  expect(dispatch(stateEvent('stopping'))).toBe('loading');
  expect(dispatch(stateEvent('recording'))).toBe('active');
});

// Keep: a reset during startup clears the latch before a later recording starts.
test('hands-free reset during startup clears the next recording tone', () => {
  const dispatch = createToneTrace();
  dispatch(stateEvent('requesting_microphone'));
  dispatch({ type: 'hands_free_changed', active: true });
  dispatch({ type: 'hands_free_changed', active: false });
  expect(dispatch(stateEvent('recording'))).toBe('active');
});

// Keep: native errors show terminal feedback; busy capture clears old feedback.
// Merge: completion identity and success presentation live in desktop-transcript-projection.test.ts.
test('overlay runtime events clear capture feedback and present terminal errors', () => {
  const state = initialOverlayState();
  let clears = 0, terminals = 0;
  const controller = createOverlayController(state, {
    reducedMotion: true, refreshPreviewWindow() {}, clearTerminalStateTimer() { clears++; },
    showTerminalStateTemporarily() { terminals++; state.terminalStateVisible = true; }
  });
  controller.handleRuntimeEvent({ type: 'error', code: 'provider_timeout', message: '' });
  expect(state.actionError).toBe('Provider timed out');
  expect(terminals).toBe(1);
  expect(overlayPresentation(state).statusTone).toBe('error');
  state.copyStatus = 'Copied';
  controller.handleRuntimeEvent(stateEvent('recording'));
  expect([state.actionError, state.copyStatus, state.terminalStateVisible, clears]).toEqual([null, null, false, 1]);
  controller.dispose();
});
