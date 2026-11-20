declare global {
  namespace App {}

  // Injected by `define` in vite.config.ts (and the test compiler) from package.json.
  const __APP_VERSION__: string;

  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

export {};

