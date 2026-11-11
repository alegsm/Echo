export type DashboardMotionEnvironment = {
  window: Pick<Window, 'matchMedia' | 'addEventListener' | 'removeEventListener'>;
  document: Pick<Document, 'hasFocus' | 'visibilityState' | 'addEventListener' | 'removeEventListener'>;
};

export type DashboardMotion = { active: boolean; reduced: boolean };

type DashboardFrameListener = (now: number, elapsed: number) => void;
const fieldFrames = new WeakMap<DashboardMotionEnvironment['document'], Set<DashboardFrameListener>>();

/** Decorative motion shares actual field draws so frost is damaged only at the field cadence. */
export function observeDashboardFrame(document: DashboardMotionEnvironment['document'], listener: DashboardFrameListener) {
  let listeners = fieldFrames.get(document);
  if (!listeners) fieldFrames.set(document, listeners = new Set());
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
    if (!listeners.size) fieldFrames.delete(document);
  };
}

export function publishDashboardFrame(document: DashboardMotionEnvironment['document'], now: number, elapsed: number) {
  fieldFrames.get(document)?.forEach((listener) => listener(now, elapsed));
}

/** Web focus and visibility are independent gates. A preference change is observed live. */
export function observeDashboardMotion(environment: DashboardMotionEnvironment, listener: (motion: DashboardMotion) => void) {
  const { window, document } = environment;
  const preference = window.matchMedia('(prefers-reduced-motion: reduce)');
  let focused = document.hasFocus();
  let previous: DashboardMotion | undefined;
  function sync() {
    const reduced = preference.matches;
    const active = focused && document.visibilityState !== 'hidden' && !reduced;
    if (previous?.active === active && previous.reduced === reduced) return;
    previous = { active, reduced };
    listener(previous);
  }
  function focus() { focused = true; sync(); }
  function blur() { focused = false; sync(); }
  window.addEventListener('focus', focus);
  window.addEventListener('blur', blur);
  document.addEventListener('visibilitychange', sync);
  preference.addEventListener('change', sync);
  sync();
  return () => {
    window.removeEventListener('focus', focus);
    window.removeEventListener('blur', blur);
    document.removeEventListener('visibilitychange', sync);
    preference.removeEventListener('change', sync);
  };
}

/** The viewport observer also accounts for clipping by each enclosing scroll container. */
export function observeDashboardIntersection(node: Element, listener: (visible: boolean) => void) {
  listener(false);
  const observer = new node.ownerDocument.defaultView!.IntersectionObserver((entries) => {
    for (const entry of entries) if (entry.target === node) listener(entry.isIntersecting);
  });
  observer.observe(node);
  return () => observer.disconnect();
}

export function dashboardGlow(node: Element) {
  let visible = false;
  let refresh = true;
  let animations: Animation[] = [];
  function refreshAnimations() { refresh = true; }
  node.addEventListener('animationstart', refreshAnimations);
  node.addEventListener('animationcancel', refreshAnimations);
  const stopFrame = observeDashboardFrame(node.ownerDocument, (_now, elapsed) => {
    if (!visible) return;
    if (refresh) {
      // Cache paused CSS animations, including pseudo-elements. Class and media changes recreate them.
      animations = node.getAnimations({ subtree: true }).filter((animation) =>
        (animation as CSSAnimation).animationName === 'echo-glow');
      refresh = false;
    }
    for (const animation of animations) animation.currentTime = elapsed;
  });
  const cleanup = observeDashboardIntersection(node, (intersecting) => {
    visible = intersecting;
    node.setAttribute('data-motion-visible', String(visible));
  });
  return () => {
    stopFrame();
    cleanup();
    node.removeEventListener('animationstart', refreshAnimations);
    node.removeEventListener('animationcancel', refreshAnimations);
    node.removeAttribute('data-motion-visible');
  };
}
