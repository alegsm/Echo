import type { EchoSnapshot } from '#lib/core/types.ts';
import type { NativeCaptureBoundary } from '#lib/runtime/desktop/native-capture-client.ts';

type DesktopStateEnvelope = { sequence: number; snapshot: EchoSnapshot };

/** Projects the native desktop snapshot without allowing late reads to roll state back. */
export function createNativeRuntimeState(boundary: NativeCaptureBoundary) {
  let current: DesktopStateEnvelope | undefined;
  let isDisposed = false;
  let unlisten: (() => void) | undefined;
  const listeners = new Set<(snapshot: EchoSnapshot) => void>();

  function apply(envelope: DesktopStateEnvelope) {
    if (isDisposed || (current && envelope.sequence <= current.sequence)) return false;
    current = envelope;
    for (const listener of listeners) listener(envelope.snapshot);
    return true;
  }

  return {
    async start() {
      if (isDisposed || unlisten) return;
      const pendingUnlisten = await boundary.listen<DesktopStateEnvelope>(
        'echo-desktop-state',
        (event) => apply(event.payload)
      );
      if (isDisposed) {
        pendingUnlisten();
        return;
      }
      unlisten = pendingUnlisten;
      apply(await boundary.invoke<DesktopStateEnvelope>('get_desktop_runtime_snapshot'));
    },
    apply,
    async refresh() {
      apply(await boundary.invoke<DesktopStateEnvelope>('get_desktop_runtime_snapshot'));
    },
    getSnapshot: () => current?.snapshot,
    subscribe(listener: (snapshot: EchoSnapshot) => void) {
      if (isDisposed) return () => undefined;
      listeners.add(listener);
      if (current) listener(current.snapshot);
      return () => listeners.delete(listener);
    },
    dispose() {
      if (isDisposed) return;
      isDisposed = true;
      unlisten?.();
      unlisten = undefined;
      listeners.clear();
    }
  };
}
