declare global {
  namespace App {}

  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

export {};

