import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { register, unregister } from '@tauri-apps/plugin-global-shortcut';
import type { FnEvent } from '#lib/runtime/desktop/hotkey-controller.ts';
import type { EchoRuntime } from '#lib/core/types.ts';
import { createDesktopRuntime } from '#lib/runtime/desktop/desktop-runtime.ts';
import { attachTauriFileLogging, detachTauriFileLogging } from '#lib/runtime/tauri-log.ts';

type TauriRuntimeOptions = { hotkeyOwner?: boolean };
type FnHotkeyPayload = FnEvent;

/** Binds Tauri APIs to the dependency-injected desktop runtime. */
export function createTauriRuntime(options: TauriRuntimeOptions = {}): EchoRuntime {
  const boundary = {
    invoke: <T>(command: string, argumentsValue?: Record<string, unknown>) =>
      invoke<T>(command, argumentsValue),
    listen: <T>(event: string, listener: (event: { payload: T }) => void) =>
      listen<T>(event, listener)
  };

  return createDesktopRuntime({
    boundary,
    hotkeyBoundary: {
      listenFn: (listener) => listen<FnHotkeyPayload>('echo-fn-hotkey', (event) => listener(event.payload)),
      register,
      setFnCaptureEnabled: (enabled) => invoke<boolean>('set_fn_hotkey_capture_enabled', { enabled }),
      unregister
    },
    hotkeyOwner: options.hotkeyOwner ?? true,
    syncDiagnostics: async (enabled) => {
      if (enabled) await attachTauriFileLogging();
      else await detachTauriFileLogging();
    }
  });
}
