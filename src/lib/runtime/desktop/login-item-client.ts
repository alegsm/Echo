export type LaunchAtLoginStatus = { registered: boolean };

type LoginItemBoundary = {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
};

/** Dashboard-only read of whether the OS will start Echo at login. Changes go through the settings commit. */
export function createLoginItemClient(boundary: LoginItemBoundary) {
  return { status: () => boundary.invoke<LaunchAtLoginStatus>('get_launch_at_login_status') };
}

export type LoginItemClient = ReturnType<typeof createLoginItemClient>;
