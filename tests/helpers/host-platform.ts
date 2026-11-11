// Server-rendered tests describe the macOS webview unless they pass a platform explicitly.
// Client harness tests replace navigator with happy-dom's and must set their own platform.
Object.defineProperty(globalThis, 'navigator', {
  configurable: true,
  writable: true,
  value: Object.create(globalThis.navigator, { platform: { configurable: true, writable: true, value: 'MacIntel' } })
});
