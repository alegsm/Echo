const FN_SHORTCUT = 'Fn';
const WINDOWS_CONTROL_SHORTCUT = 'Control+Windows';
const LEGACY_UNREGISTERABLE_SHORTCUTS = new Set(['Ctrl+Opt', 'Control+Option']);
const TAP_MAX_MS = 300;
const RELEASE_DEBOUNCE_MS = 80;

export type HotkeyClock = {
  now(): number;
  schedule(callback: () => void, delayMs: number): () => void;
};

const monotonicClock: HotkeyClock = {
  now: () => performance.now(),
  schedule(callback, delayMs) {
    const timer = setTimeout(callback, delayMs);
    return () => clearTimeout(timer);
  }
};

type HotkeyState =
  | { phase: 'idle' }
  | { phase: 'hands-free' }
  | { phase: 'initial-held' | 'finishing-held'; pressedAt: number; releasedAt?: number };
type HotkeyInput = { type: 'press' | 'release' | 'debounce'; at: number };
type HotkeyEffect = 'start' | 'stop' | 'latch';
type HotkeyTransition = { state: HotkeyState; effect?: HotkeyEffect };

/** Pure edge transitions; timestamps always describe physical edges, not timer delivery. */
function transitionHotkey(state: HotkeyState, input: HotkeyInput): HotkeyTransition {
  if (input.type === 'press') {
    if (state.phase === 'idle') {
      return { state: { phase: 'initial-held', pressedAt: input.at }, effect: 'start' };
    }
    if (state.phase === 'hands-free') {
      return { state: { phase: 'finishing-held', pressedAt: input.at } };
    }
    if (state.releasedAt !== undefined) {
      return { state: { phase: state.phase, pressedAt: state.pressedAt } };
    }
    return { state };
  }
  if (state.phase === 'idle' || state.phase === 'hands-free') return { state };
  if (input.type === 'release') {
    return state.releasedAt === undefined
      ? { state: { ...state, releasedAt: input.at } }
      : { state };
  }
  if (state.releasedAt === undefined || input.at < state.releasedAt + RELEASE_DEBOUNCE_MS) {
    return { state };
  }
  if (state.phase === 'initial-held' && state.releasedAt - state.pressedAt <= TAP_MAX_MS) {
    return { state: { phase: 'hands-free' }, effect: 'latch' };
  }
  return { state: { phase: 'idle' }, effect: 'stop' };
}

type ShortcutEvent = { state: 'Pressed' | 'Released' };
export type FnEvent = { state?: 'pressed' | 'released' | 'available' | 'unavailable' };

export type HotkeyBoundary = {
  listenFn(listener: (event: FnEvent) => void): Promise<() => void>;
  register(shortcut: string, listener: (event: ShortcutEvent) => void): Promise<void>;
  setFnCaptureEnabled(enabled: boolean): Promise<void | boolean>;
  unregister(shortcut: string): Promise<void>;
};

export class HotkeyConfigurationError extends Error {
  constructor(reason: 'missing_fallback' | 'unregister_failed') {
    super(reason === 'missing_fallback'
      ? 'Native shortcut unavailable. Choose a fallback shortcut in Settings.'
      : 'The previous shortcut could not be removed. Try changing the shortcut again.');
  }
}

type HotkeyControllerOptions = {
  boundary: HotkeyBoundary;
  clock?: HotkeyClock;
  onCancel(): void | Promise<void>;
  onHandsFreeChanged?(active: boolean): void;
  onError?(error: unknown): void;
  onPress(): void | boolean | Promise<void | boolean>;
  onRelease(): void | Promise<void>;
};

/** Owns tap/hold intent, debounce and registration for every desktop shortcut source. */
export function createHotkeyController(options: HotkeyControllerOptions) {
  return new HotkeyController(options);
}

class HotkeyController {
  private activeShortcut: string | undefined;
  private registrationGeneration = 0;
  private configuredFallback: string | undefined;
  private configuredPrimary: string | undefined;
  private requestedPrimary: string | undefined;
  private requestedFallback: string | undefined;
  private configurationTail = Promise.resolve<string | undefined>(undefined);
  private configurationRevision = 0;
  private nativeAvailable = true;
  private configuredNativeAvailable = true;
  private isDisposed = false;
  private state: HotkeyState = { phase: 'idle' };
  private generation = 0;
  private suppressUntilRelease = false;
  private cancelTimer: (() => void) | undefined;
  private readonly clock: HotkeyClock;
  private unlistenFn: (() => void) | undefined;

  constructor(private readonly options: HotkeyControllerOptions) {
    this.clock = options.clock ?? monotonicClock;
  }

  async start() {
    if (this.isDisposed || this.unlistenFn) return;
    const pendingUnlisten = await this.options.boundary.listenFn((event) => {
      if (event.state === 'available' || event.state === 'unavailable') {
        this.nativeAvailable = event.state === 'available';
        if (isNativeModifierShortcut(this.requestedPrimary)) {
          void this.configure(this.requestedPrimary, this.requestedFallback).catch((error: unknown) => {
            this.options.onError?.(error);
          });
        }
        return;
      }
      if (!this.nativeAvailable || !isNativeModifierShortcut(this.activeShortcut)) return;
      if (event.state === 'pressed') this.press();
      if (event.state === 'released') this.release();
    });
    if (this.isDisposed) pendingUnlisten();
    else this.unlistenFn = pendingUnlisten;
  }

  async configure(primaryValue: string | undefined, fallbackValue?: string) {
    if (this.isDisposed) return undefined;
    this.requestedPrimary = normalizeShortcut(primaryValue);
    this.requestedFallback = normalizeShortcut(fallbackValue);
    const revision = ++this.configurationRevision;
    const configured = this.configurationTail.then(() =>
      this.applyConfiguration(primaryValue, fallbackValue, revision)
    );
    this.configurationTail = configured.catch(() => undefined);
    return configured;
  }

  private async applyConfiguration(
    primaryValue: string | undefined,
    fallbackValue: string | undefined,
    revision: number
  ) {
    const primary = normalizeShortcut(primaryValue);
    const fallback = normalizeShortcut(fallbackValue);
    if (primary === this.configuredPrimary && fallback === this.configuredFallback && this.nativeAvailable === this.configuredNativeAvailable) {
      return this.activeShortcut;
    }
    this.configuredPrimary = undefined;
    this.configuredFallback = undefined;
    await this.cancelIntent();
    await this.clearActiveShortcut();
    this.suppressUntilRelease = false;
    if (this.configurationChanged(revision)) return undefined;
    if (!primary) {
      await this.options.boundary.setFnCaptureEnabled(false);
      if (!this.configurationChanged(revision)) this.rememberConfiguration(primary, fallback);
      return undefined;
    }
    if (isNativeModifierShortcut(primary)) {
      const active = await this.configureNative(primary, fallback, revision);
      if (!this.configurationChanged(revision)) this.rememberConfiguration(primary, fallback);
      return active;
    }
    await this.options.boundary.setFnCaptureEnabled(false);
    try {
      if (await this.registerGlobal(primary, revision)) {
        this.rememberConfiguration(primary, fallback);
        return primary;
      }
    } catch (primaryError) {
      if (!fallback || fallback === primary) throw primaryError;
      if (await this.registerGlobal(fallback, revision)) {
        this.rememberConfiguration(primary, fallback);
        return fallback;
      }
    }
    return undefined;
  }

  getActiveShortcut() {
    return this.activeShortcut;
  }

  isHandsFree() {
    return this.state.phase === 'hands-free' || this.state.phase === 'finishing-held';
  }

  hasIntent() {
    return this.state.phase !== 'idle';
  }

  /** Invalidates callbacks before a terminal capture action can notify subscribers. */
  reset(suppressHeld = false) {
    this.suppressUntilRelease ||= suppressHeld && (this.state.phase === 'initial-held' || this.state.phase === 'finishing-held')
      && this.state.releasedAt === undefined;
    this.generation += 1;
    this.cancelTimer?.();
    this.cancelTimer = undefined;
    this.state = { phase: 'idle' };
    this.options.onHandsFreeChanged?.(false);
  }

  async dispose() {
    if (this.isDisposed) return;
    this.isDisposed = true;
    const cancelling = this.cancelIntent();
    this.configurationRevision += 1;
    this.unlistenFn?.();
    this.unlistenFn = undefined;
    await this.configurationTail;
    await this.clearActiveShortcut().catch(() => undefined);
    await this.options.boundary.setFnCaptureEnabled(false).catch(() => undefined);
    await cancelling;
  }

  private press() {
    this.edge('press');
  }

  private release() {
    this.edge('release');
  }

  private edge(type: 'press' | 'release') {
    if (this.isDisposed) return;
    if (this.suppressUntilRelease) {
      if (type === 'release') this.suppressUntilRelease = false;
      return;
    }
    const at = this.clock.now();
    this.dispatch({ type: 'debounce', at });
    this.dispatch({ type, at });
  }

  private dispatch(input: HotkeyInput) {
    const previous = this.state;
    const { state, effect } = transitionHotkey(previous, input);
    if (state === previous) return;
    this.cancelTimer?.();
    this.cancelTimer = undefined;
    this.state = state;
    if ('releasedAt' in state && state.releasedAt !== undefined) {
      const generation = this.generation;
      const deadline = state.releasedAt + RELEASE_DEBOUNCE_MS;
      this.cancelTimer = this.clock.schedule(() => {
        if (this.isDisposed || generation !== this.generation || this.state !== state) return;
        // Timer delivery can precede the deadline on the coarsened monotonic clock.
        this.dispatch({ type: 'debounce', at: Math.max(this.clock.now(), deadline) });
      }, RELEASE_DEBOUNCE_MS);
    }
    if (effect === 'start') {
      const generation = ++this.generation;
      let starting: void | boolean | Promise<void | boolean>;
      try {
        starting = this.options.onPress();
      } catch {
        void this.cancelIntent();
        return;
      }
      void Promise.resolve(starting).then((started) => {
        if (started === false && generation === this.generation) return this.cancelIntent();
      }).catch(() => {
        if (generation === this.generation) return this.cancelIntent();
      });
    }
    if (effect === 'latch') this.options.onHandsFreeChanged?.(true);
    if (effect === 'stop') {
      this.reset();
      void this.options.onRelease();
    }
  }

  private async configureNative(shortcut: string, fallback: string | undefined, revision: number): Promise<string | undefined> {
    // Query health while capture is disabled. Recovery removes the old fallback first.
    const available = await this.options.boundary.setFnCaptureEnabled(false);
    if (this.configurationChanged(revision)) return undefined;
    this.nativeAvailable = available ?? this.nativeAvailable;
    if (this.nativeAvailable) {
      const enabled = await this.options.boundary.setFnCaptureEnabled(true);
      if (this.configurationChanged(revision)) {
        await this.options.boundary.setFnCaptureEnabled(false).catch(() => undefined);
        return undefined;
      }
      this.nativeAvailable = enabled ?? this.nativeAvailable;
      if (this.nativeAvailable) {
        this.activeShortcut = shortcut;
        return shortcut;
      }
    }
    if (!fallback || isNativeModifierShortcut(fallback)) {
      throw new HotkeyConfigurationError('missing_fallback');
    }
    await this.registerGlobal(fallback, revision, true);
    return this.activeShortcut;
  }

  private async registerGlobal(shortcut: string, revision: number, toggle = false) {
    let held = false;
    const registration = ++this.registrationGeneration;
    await this.options.boundary.register(shortcut, (event) => {
      if (this.isDisposed || this.activeShortcut !== shortcut || registration !== this.registrationGeneration) return;
      if (!toggle) {
        if (event.state === 'Pressed') this.press();
        if (event.state === 'Released') this.release();
        return;
      }
      if (event.state === 'Released') { held = false; return; }
      if (held) return;
      held = true;
      if (this.hasIntent()) {
        this.reset();
        void this.options.onRelease();
      } else {
        this.press();
        this.release();
        this.dispatch({ type: 'debounce', at: this.clock.now() + RELEASE_DEBOUNCE_MS });
      }
    });
    this.activeShortcut = shortcut;
    if (this.configurationChanged(revision)) {
      await this.clearActiveShortcut();
      return false;
    }
    return true;
  }

  private async clearActiveShortcut() {
    this.registrationGeneration += 1;
    const previous = this.activeShortcut;
    if (previous && !isNativeModifierShortcut(previous)) {
      try {
        await this.options.boundary.unregister(previous);
      } catch {
        throw new HotkeyConfigurationError('unregister_failed');
      }
    }
    this.activeShortcut = undefined;
  }

  private async cancelIntent() {
    if (!this.hasIntent()) return;
    this.reset();
    await this.options.onCancel();
  }

  private rememberConfiguration(primary: string | undefined, fallback: string | undefined) {
    this.configuredNativeAvailable = this.nativeAvailable;
    this.configuredPrimary = primary;
    this.configuredFallback = fallback;
  }

  private configurationChanged(revision: number) {
    return this.isDisposed || revision !== this.configurationRevision;
  }
}

function normalizeShortcut(value: string | undefined) {
  const shortcut = value?.trim();
  if (!shortcut) return undefined;
  if (LEGACY_UNREGISTERABLE_SHORTCUTS.has(shortcut)) return FN_SHORTCUT;
  const lowerShortcut = shortcut.toLowerCase();
  if (lowerShortcut === 'fn' || lowerShortcut === 'function') return FN_SHORTCUT;
  if (['control+windows', 'ctrl+windows', 'control+super', 'ctrl+super'].includes(lowerShortcut)) {
    return WINDOWS_CONTROL_SHORTCUT;
  }
  return shortcut;
}

function isNativeModifierShortcut(shortcut: string | undefined) {
  return shortcut === FN_SHORTCUT || shortcut === WINDOWS_CONTROL_SHORTCUT;
}
