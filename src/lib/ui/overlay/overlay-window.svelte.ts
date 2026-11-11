import { invoke } from '@tauri-apps/api/core';
import { LogicalSize } from '@tauri-apps/api/dpi';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import { getCurrentWindow, type Window as TauriWindow } from '@tauri-apps/api/window';
import { OverlayWindowShapes } from './overlay-window-shape';

type OverlayWindowOptions = { onError(message: string): void };
type DragStart = { dragging: boolean; x: number; y: number };

/** Owns Tauri overlay sizing and pointer-drag transitions. */
export function createOverlayWindowController(options: OverlayWindowOptions) {
  return new OverlayWindowController(options);
}

class OverlayWindowController {
  expanded = $state(false);
  previewReady = $state(false);
  private dragStart: DragStart | undefined;
  private suppressNextClick = false;
  private window: TauriWindow | undefined;
  private readonly shapes: OverlayWindowShapes;

  constructor(options: OverlayWindowOptions) {
    this.shapes = new OverlayWindowShapes({
      onError: options.onError,
      onChanged: (state) => {
        this.expanded = state.expanded;
        this.previewReady = state.previewReady;
      }
    });
  }

  async initialize() {
    if (typeof window === 'undefined' || !(window as any).__TAURI_INTERNALS__) return;
    if (getCurrentWebviewWindow().label !== 'overlay') return;
    this.window = getCurrentWindow();
    void this.window.setBackgroundColor([0, 0, 0, 0]).catch(() => undefined);
    void getCurrentWebview().setAutoResize(false).catch(() => undefined);
    const nativeWindow = this.window;
    await this.shapes.initialize({
      setSize: ({ width, height }) => nativeWindow.setSize(new LogicalSize(width, height)),
      position: (placement) => invoke('position_overlay', placement),
      show: () => nativeWindow.show()
    });
  }

  setPreview(retained: boolean) {
    return this.shapes.setPreview(retained);
  }

  async toggle(event: MouseEvent) {
    event.stopPropagation();
    if (this.suppressNextClick) {
      this.suppressNextClick = false;
      return;
    }
    await this.shapes.setExpanded(!this.expanded);
  }

  collapseFrom(event?: MouseEvent) {
    event?.stopPropagation();
    return this.shapes.setExpanded(false);
  }

  handleDragStart(event: PointerEvent) {
    if (this.expanded || event.button !== 0) return;
    this.dragStart = { x: event.clientX, y: event.clientY, dragging: false };
  }

  async handleDragMove(event: PointerEvent) {
    if (!this.dragStart || this.dragStart.dragging || this.expanded) return;
    const deltaX = Math.abs(event.clientX - this.dragStart.x);
    const deltaY = Math.abs(event.clientY - this.dragStart.y);
    if (deltaX < 4 && deltaY < 4) return;
    this.dragStart.dragging = true;
    this.suppressNextClick = true;
    await this.window?.startDragging().catch(() => undefined);
  }

  handleDragEnd() {
    this.dragStart = undefined;
  }
}
