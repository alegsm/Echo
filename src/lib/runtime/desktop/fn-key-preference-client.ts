export type FnKeyPreference = {
  supported: boolean;
  current: number | 'unrecognized' | null;
  canRestore: boolean;
};

export type FnKeyAction = 'use_fn' | 'restore' | 'system_default';

type FnKeyBoundary = {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
};

/** Dashboard-only preference operations. No renderer-supplied preference value crosses IPC. */
export function createFnKeyPreferenceClient(boundary: FnKeyBoundary) {
  const read = () => boundary.invoke<FnKeyPreference>('get_fn_key_preference');
  return {
    read,
    async change(action: FnKeyAction) {
      await boundary.invoke('change_fn_key_preference', { action });
      return read();
    }
  };
}
