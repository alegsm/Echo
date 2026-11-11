import type { DashboardContext, DesktopPermissions, PermissionKind } from '../context';

export type PermissionsState = {
  permissionRefreshState: 'idle' | 'checking';
  permissionAction: PermissionKind | undefined;
};

export function desktopPlatform(permissions?: DesktopPermissions): string {
  if (permissions?.platform) return permissions.platform;
  if (typeof navigator !== 'undefined' && /win/i.test(`${navigator.platform} ${navigator.userAgent}`)) return 'windows';
  return 'macos';
}

export function applicablePermissionKinds(permissions?: DesktopPermissions): readonly PermissionKind[] {
  const kinds: readonly PermissionKind[] = desktopPlatform(permissions) === 'windows'
    ? ['microphone'] : ['microphone', 'accessibility', 'input_monitoring'];
  return kinds.filter((kind) => permissions?.[kind]?.status !== 'not_required');
}

type PermissionsContext = Pick<DashboardContext, 'refreshPermissions' | 'runtime' | 'setActionError' | 'setPermissions'>
  & Partial<Pick<DashboardContext, 'permissions'>>;
type PermissionEffects = {
  tick: () => Promise<void>;
  activeElement: () => Element | null;
  focusHeading: () => void;
  refreshFn: () => Promise<void>;
};

export function createPermissionsController(ctx: PermissionsContext, state: PermissionsState, effects: PermissionEffects) {
  async function doRefresh() {
    if (state.permissionRefreshState === 'checking' || state.permissionAction) return;
    state.permissionRefreshState = 'checking';
    await Promise.all([
      ctx.refreshPermissions().finally(() => { state.permissionRefreshState = 'idle'; }),
      effects.refreshFn()
    ]);
  }

  async function requestPermission(kind: PermissionKind, trigger?: HTMLButtonElement) {
    if (state.permissionAction || state.permissionRefreshState === 'checking') return;
    const hadFocus = trigger && effects.activeElement() === trigger;
    state.permissionAction = kind;
    ctx.setActionError(null);
    try {
      const result = await ctx.runtime()?.requestDesktopPermission(kind);
      if (result) ctx.setPermissions(result);
      if (kind === 'microphone' && result?.microphone?.granted === false) {
        await openSettings(kind);
      }
    } catch {
      ctx.setActionError('Permission could not be changed. Re-check or try again.');
    } finally {
      state.permissionAction = undefined;
      await effects.tick();
      if (hadFocus && !trigger.isConnected) effects.focusHeading();
    }
  }

  async function openPermissionSettings(kind: PermissionKind) {
    if (state.permissionAction || state.permissionRefreshState === 'checking') return;
    state.permissionAction = kind;
    ctx.setActionError(null);
    try {
      await openSettings(kind);
    } finally {
      state.permissionAction = undefined;
    }
  }

  async function openSettings(kind: PermissionKind) {
    try {
      await ctx.runtime()?.openDesktopPermissionSettings(kind);
    } catch {
      const settings = desktopPlatform(ctx.permissions?.()) === 'windows' ? 'Settings' : 'System Settings';
      ctx.setActionError(`Could not open ${settings}. Try again.`);
    }
  }

  return { doRefresh, requestPermission, openPermissionSettings };
}
