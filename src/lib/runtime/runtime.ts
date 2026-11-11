import { createTauriRuntime } from './tauri-runtime';
import type { EchoRuntime } from '#lib/core/types.ts';

type EchoRuntimeOptions = {
  hotkeyOwner?: boolean;
};

export function createEchoRuntime(options: EchoRuntimeOptions = {}): EchoRuntime {
  if (typeof window === 'undefined' || !window.__TAURI_INTERNALS__) {
    throw new Error('Echo requires the Tauri desktop app.');
  }
  return createTauriRuntime({ hotkeyOwner: options.hotkeyOwner ?? true });
}
