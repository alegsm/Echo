export const MORPH_MS = 320;

export type ShellPhase = {
  phase: 'resting' | 'outward' | 'landed' | 'returning';
  generation: number;
};

export const emptyShell: ShellPhase = { phase: 'resting', generation: 0 };

type ShellEvent =
  | { type: 'sync'; hasText: boolean; ready: boolean; reducedMotion?: boolean }
  | { type: 'finished'; generation: number }
  | { type: 'released'; reducedMotion?: boolean };

/** Text and release own the destination; native placement gates the first outward leg. */
export function reduceShell(state: ShellPhase, event: ShellEvent): ShellPhase {
  if (event.type === 'finished') {
    if (event.generation !== state.generation) return state;
    if (state.phase === 'outward') return { ...state, phase: 'landed' };
    if (state.phase === 'returning') return { ...state, phase: 'resting' };
    return state;
  }
  if (event.type === 'released') {
    if (state.phase !== 'outward' && state.phase !== 'landed') return state;
    return { phase: event.reducedMotion ? 'resting' : 'returning', generation: state.generation + 1 };
  }
  let phase = state.phase;
  if (!event.hasText) {
    if (phase === 'outward' || phase === 'landed') phase = 'returning';
  } else if (event.ready && (phase === 'resting' || phase === 'returning')) {
    phase = 'outward';
  }
  if (event.reducedMotion) {
    if (phase === 'outward') phase = 'landed';
    if (phase === 'returning') phase = 'resting';
  }
  return phase === state.phase ? state : { phase, generation: state.generation + 1 };
}

/** Keep the orb's screen cell stable until the visual return has completed. */
export function retainsPreviewWindow(shell: ShellPhase, hasText: boolean): boolean {
  return hasText || shell.phase !== 'resting';
}
