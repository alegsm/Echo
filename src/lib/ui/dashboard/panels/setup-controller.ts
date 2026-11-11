import type { DashboardContext } from '../context';

export function createSetupController(ctx: Pick<DashboardContext, 'setActiveTab'>, state: { step: number }, effects: {
  tick: () => Promise<void>; focusHeading: () => void;
}) {
  async function setStep(next: number) {
    state.step = next;
    await effects.tick();
    effects.focusHeading();
  }

  function finishSetup() {
    ctx.setActiveTab('history');
  }

  return { setStep, finishSetup };
}
