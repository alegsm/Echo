import type { DashboardContext } from '../context';
import type { LoginItemClient } from '#lib/runtime/desktop/login-item-client.ts';
import { desktopPlatform } from './permissions-controller';

type BackgroundSetting = 'runInBackground' | 'launchAtLogin';
/** `registered` is the last status read, or null while unknown, being re-read or unreadable. */
export type BackgroundState = { saving: boolean; loginError: string | null; registered: boolean | null };
type BackgroundContext = Pick<DashboardContext, 'settings' | 'permissions' | 'setActionError' | 'updateSettings'>;

// Native rejects a refused login registration with this prefix and the OS detail.
const LOGIN_ITEM_FAILED = 'login_item_failed: ';

// Settings unmounts on navigation; keying the card state to the dashboard context keeps an
// in-flight save guarded when the card mounts again, and lets that save's status re-read
// reach whichever card is mounted when it finishes.
type SharedBackground = { state: BackgroundState; statusSequence: number; refreshStatus?: () => Promise<void> };
const shared = new WeakMap<BackgroundContext, SharedBackground>();

export function createBackgroundController(ctx: BackgroundContext, initial: BackgroundState, loginItem?: LoginItemClient) {
  const entry = shared.get(ctx) ?? { state: initial, statusSequence: 0 };
  shared.set(ctx, entry);
  const { state } = entry;
  let mounted = false;

  // Only the newest read issued while this card is mounted may change it.
  async function refreshStatus() {
    if (!loginItem || !mounted) return;
    const sequence = ++entry.statusSequence;
    let registered: boolean | null;
    try {
      registered = (await loginItem.status()).registered;
    } catch {
      registered = null;
    }
    if (mounted && sequence === entry.statusSequence) state.registered = registered;
  }

  function invalidateStatus() {
    entry.statusSequence++;
    state.registered = null;
  }

  function reportSaveFailure(setting: BackgroundSetting, turningOn: boolean, error: unknown) {
    if (setting === 'launchAtLogin' && typeof error === 'string' && error.startsWith(LOGIN_ITEM_FAILED)) {
      state.loginError = `Start Echo at login could not be turned ${turningOn ? 'on' : 'off'}: ${error.slice(LOGIN_ITEM_FAILED.length)}`;
      return;
    }
    ctx.setActionError('Settings could not be saved. Try again.');
  }

  // One pending save covers both switches. Status is re-read after every launchAtLogin save.
  async function toggle(setting: BackgroundSetting) {
    if (state.saving) return;
    state.saving = true;
    ctx.setActionError(null);
    state.loginError = null;
    const next = !ctx.settings()[setting];
    if (setting === 'launchAtLogin') invalidateStatus();
    try {
      await ctx.updateSettings({ [setting]: next });
    } catch (error) {
      reportSaveFailure(setting, next, error);
    } finally {
      state.saving = false;
      if (setting === 'launchAtLogin') void entry.refreshStatus?.();
    }
  }

  /** The line shown when the OS and the saved setting disagree, or null. */
  function mismatch() {
    if (state.registered === null) return null;
    const saved = ctx.settings().launchAtLogin;
    if (saved === state.registered) return null;
    const os = desktopPlatform(ctx.permissions()) === 'windows' ? 'Windows' : 'macOS';
    return saved
      ? `Not registered with ${os}. Turn the switch off and on again.`
      : `${os} still starts Echo at login. Turn the switch on and off again.`;
  }

  function mount() {
    mounted = true;
    entry.refreshStatus = refreshStatus;
    void refreshStatus();
    return () => {
      mounted = false;
      if (entry.refreshStatus === refreshStatus) entry.refreshStatus = undefined;
    };
  }

  return { state, toggle, mismatch, mount };
}
