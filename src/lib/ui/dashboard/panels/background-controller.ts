import type { DashboardContext } from '../context';

type BackgroundSetting = 'runInBackground' | 'launchAtLogin';
export type BackgroundState = { saving: boolean };
type BackgroundContext = Pick<DashboardContext, 'settings' | 'setActionError' | 'updateSettings'>;

// Settings unmounts on navigation; keying the pending flag to the dashboard context keeps
// an in-flight save guarded when the card mounts again.
const pendingSaves = new WeakMap<BackgroundContext, BackgroundState>();

export function createBackgroundController(ctx: BackgroundContext, initial: BackgroundState) {
  const state = pendingSaves.get(ctx) ?? initial;
  pendingSaves.set(ctx, state);

  // The single route for Background save failures, so login-registration errors can later be sent to the card here.
  function reportSaveFailure() {
    ctx.setActionError('Settings could not be saved. Try again.');
  }

  // One pending save covers both switches.
  async function toggle(setting: BackgroundSetting) {
    if (state.saving) return;
    state.saving = true;
    ctx.setActionError(null);
    try {
      await ctx.updateSettings({ [setting]: !ctx.settings()[setting] });
    } catch {
      reportSaveFailure();
    } finally {
      state.saving = false;
    }
  }

  return { state, toggle };
}
