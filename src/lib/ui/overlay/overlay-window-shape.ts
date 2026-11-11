export type OverlaySize = { width: number; height: number };
export type OverlayPlacement = OverlaySize & {
  bottomGap: number;
  keepCell: boolean;
  sourceCellOffset: { x: number; y: number };
};

type Shape = 'compact' | 'preview' | 'expanded';
type NativeWindow = {
  setSize(size: OverlaySize): Promise<void>;
  position(placement: OverlayPlacement): Promise<void>;
  show(): Promise<void>;
};
type ShapeState = { expanded: boolean; previewReady: boolean };
type ShapeOptions = {
  onChanged(state: ShapeState): void;
  onError(message: string): void;
};

const SHAPES = {
  compact: { width: 80, height: 80, cellOffset: { x: 0, y: 0 } },
  preview: { width: 380, height: 144, cellOffset: { x: 150, y: 64 } },
  expanded: { width: 380, height: 210, cellOffset: { x: 150, y: 130 } }
};

/** Serializes native geometry and exposes only placement-complete preview readiness. */
export class OverlayWindowShapes {
  expanded = false;
  private preview = false;
  private committed: Shape | undefined;
  // Placement may succeed before show fails; its cell offset still owns the next transition.
  private placed: Shape | undefined;
  private native: NativeWindow | undefined;
  private running: Promise<void> | undefined;

  constructor(private readonly options: ShapeOptions) {}

  get previewReady() {
    return this.requested === 'preview' && this.committed === 'preview';
  }

  private get requested(): Shape {
    return this.expanded ? 'expanded' : this.preview ? 'preview' : 'compact';
  }

  initialize(native: NativeWindow) {
    this.native = native;
    return this.reconcile();
  }

  setPreview(preview: boolean) {
    this.preview = preview;
    return this.reconcile();
  }

  setExpanded(expanded: boolean) {
    this.expanded = expanded;
    return this.reconcile();
  }

  private publish() {
    this.options.onChanged({ expanded: this.expanded, previewReady: this.previewReady });
  }

  private reconcile() {
    this.publish();
    if (!this.native) return Promise.resolve();
    if (!this.running) {
      if (this.committed === this.requested) return Promise.resolve();
      this.running = this.resizeUntilCurrent(this.native);
    }
    return this.running;
  }

  private async resizeUntilCurrent(native: NativeWindow) {
    try {
      while (this.committed !== this.requested) {
        const target = this.requested;
        const { width, height } = SHAPES[target];
        this.committed = undefined;
        this.publish();
        try {
          // Yield before native code can throw so the worker is registered before cleanup.
          await Promise.resolve().then(() => native.setSize({ width, height }));
          await native.position({
            width, height, bottomGap: 0,
            keepCell: this.placed !== undefined,
            sourceCellOffset: SHAPES[this.placed ?? 'compact'].cellOffset
          });
          this.placed = target;
          await native.show();
          this.committed = target;
          this.publish();
        } catch (error) {
          this.options.onError(error instanceof Error ? error.message : 'Window resize failed');
          if (this.requested === target) {
            if (target !== 'expanded') return;
            this.expanded = false;
            this.publish();
          }
        }
      }
    } finally {
      // Clear before resolving; requests queued at commit must start a new worker.
      this.running = undefined;
    }
  }
}
