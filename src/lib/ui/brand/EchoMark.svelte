<script module lang="ts">
  export const MARK_SHINE_PERIOD_MS = 5000;
  const SHINE_DURATION_MS = 1400;
</script>

<script lang="ts">
  let { shinePeriodMs = 0, motionAllowed = true, observeFrame }: {
    shinePeriodMs?: number;
    motionAllowed?: boolean;
    observeFrame?: (listener: FrameRequestCallback) => () => void;
  } = $props();
  const REST_OFFSET = -523.293;
  const START_OFFSET = 324.185;
  let headOffset = $state(REST_OFFSET);

  $effect(() => {
    const period = shinePeriodMs;
    if (period <= 0 || !motionAllowed) return;
    const motion = window.matchMedia('(prefers-reduced-motion: reduce)');
    let timer: ReturnType<typeof setTimeout>;
    let frame = 0;
    let passStart = 0;
    let shining = false;
    const sharedFrame = observeFrame;

    function canShine() {
      return !motion.matches && document.visibilityState !== 'hidden';
    }

    function tick(now: number) {
      if (!canShine()) { sync(); return; }
      const progress = Math.min(1, (now - passStart) / SHINE_DURATION_MS);
      const eased = 1 - (1 - progress) ** 3;
      headOffset = progress === 1 ? REST_OFFSET : START_OFFSET + (REST_OFFSET - START_OFFSET) * eased;
      if (progress < 1) {
        if (!sharedFrame) frame = requestAnimationFrame(tick);
      }
      else {
        shining = false;
        frame = 0;
        timer = setTimeout(shine, Math.max(0, period - (now - passStart)));
      }
    }

    function shine() {
      if (!canShine()) { sync(); return; }
      passStart = performance.now();
      shining = true;
      if (!sharedFrame) {
        headOffset = START_OFFSET;
        frame = requestAnimationFrame(tick);
      }
    }

    function sync() {
      clearTimeout(timer);
      cancelAnimationFrame(frame);
      frame = 0;
      shining = false;
      headOffset = REST_OFFSET;
      if (canShine()) timer = setTimeout(shine, period);
    }

    const stopFrame = sharedFrame?.((now) => { if (shining) tick(now); });
    sync();
    motion.addEventListener('change', sync);
    document.addEventListener('visibilitychange', sync);
    return () => {
      clearTimeout(timer);
      stopFrame?.();
      cancelAnimationFrame(frame);
      motion.removeEventListener('change', sync);
      document.removeEventListener('visibilitychange', sync);
      headOffset = REST_OFFSET;
    };
  });
  const id = $props.id();
  const path = 'M25 50 C25.51 46.86 26.01 43.96 26.52 41.63 C27.02 39.3 27.53 37.55 28.03 36.52 C28.54 35.49 29.04 35.18 29.55 35.53 C30.05 35.88 30.56 36.89 31.06 38.35 C31.57 39.8 32.07 41.7 32.58 43.74 C33.08 45.79 33.59 47.97 34.09 50 C34.6 52.03 35.1 53.91 35.61 55.41 C36.11 56.91 36.62 58.05 37.12 58.71 C37.63 59.38 38.13 59.58 38.64 59.35 C39.14 59.13 39.65 58.47 40.15 57.53 C40.66 56.59 41.16 55.36 41.67 54.04 C42.17 52.72 42.68 51.31 43.18 50 C43.69 48.69 44.19 47.48 44.7 46.5 C45.2 45.53 45.71 44.8 46.21 44.37 C46.72 43.94 47.22 43.81 47.73 43.95 C48.23 44.1 48.74 44.52 49.24 45.13 C49.75 45.74 50.25 46.53 50.76 47.39 C51.26 48.24 51.77 49.15 52.27 50 C52.78 50.85 53.28 51.63 53.79 52.26 C54.29 52.89 54.8 53.36 55.3 53.64 C55.81 53.92 56.31 54 56.82 53.91 C57.32 53.81 57.83 53.54 58.33 53.15 C58.84 52.75 59.34 52.24 59.85 51.69 C60.35 51.14 60.86 50.55 61.36 50 C61.87 49.45 62.37 48.95 62.88 48.54 C63.38 48.13 63.89 47.83 64.39 47.65 C64.9 47.47 65.4 47.41 65.91 47.47 C66.41 47.53 66.92 47.71 67.42 47.97 C67.93 48.22 68.43 48.55 68.94 48.91 C69.44 49.26 69.95 49.65 70.45 50 C70.96 50.35 71.46 50.68 71.97 50.94 C72.47 51.21 72.98 51.4 73.48 51.52 C73.99 51.64 74.49 51.67 75 51.63';

  // The supplied nested dashes share one offset; the head ends at the frozen master pose.
  const trail = [
    ["0 316.08 8.105 1648.37", "1"],
    ["0 307.976 16.209 1648.37", "0.5"],
    ["0 299.871 24.314 1648.37", "0.333"],
    ["0 291.766 32.418 1648.37", "0.25"],
    ["0 283.662 40.523 1648.37", "0.2"],
    ["0 275.557 48.628 1648.37", "0.167"],
    ["0 267.452 56.732 1648.37", "0.143"],
    ["0 259.348 64.837 1648.37", "0.125"],
    ["0 251.243 72.942 1648.37", "0.111"],
    ["0 243.139 81.046 1648.37", "0.1"],
    ["0 235.034 89.151 1648.37", "0.091"],
    ["0 226.929 97.255 1648.37", "0.083"],
    ["0 218.825 105.36 1648.37", "0.077"],
    ["0 210.72 113.465 1648.37", "0.071"],
    ["0 202.616 121.569 1648.37", "0.067"],
    ["0 194.511 129.674 1648.37", "0.063"],
    ["0 186.406 137.779 1648.37", "0.059"],
    ["0 178.302 145.883 1648.37", "0.056"],
    ["0 170.197 153.988 1648.37", "0.053"],
    ["0 162.092 162.092 1648.37", "0.05"],
    ["0 153.988 170.197 1648.37", "0.048"],
    ["0 145.883 178.302 1648.37", "0.045"],
    ["0 137.779 186.406 1648.37", "0.043"],
    ["0 129.674 194.511 1648.37", "0.042"],
    ["0 121.569 202.616 1648.37", "0.04"],
    ["0 113.465 210.72 1648.37", "0.038"],
    ["0 105.36 218.825 1648.37", "0.037"],
    ["0 97.255 226.929 1648.37", "0.036"],
    ["0 89.151 235.034 1648.37", "0.034"],
    ["0 81.046 243.139 1648.37", "0.033"],
    ["0 72.942 251.243 1648.37", "0.032"],
    ["0 64.837 259.348 1648.37", "0.031"],
    ["0 56.732 267.452 1648.37", "0.03"],
    ["0 48.628 275.557 1648.37", "0.029"],
    ["0 40.523 283.662 1648.37", "0.029"],
    ["0 32.418 291.766 1648.37", "0.028"],
    ["0 24.314 299.871 1648.37", "0.027"],
    ["0 16.209 307.976 1648.37", "0.026"],
    ["0 8.105 316.08 1648.37", "0.026"],
    ["0 0 324.185 1648.37", "0.025"],
  ] as const;
</script>

<svg xmlns="http://www.w3.org/2000/svg" viewBox="22 30 56 40" width="48" height="48" fill="none" aria-hidden="true" focusable="false">
  <defs>
    <filter id={`${id}-line`} filterUnits="userSpaceOnUse" x="-10" y="-10" width="120" height="120">
      <feDropShadow dx="0" dy="0" stdDeviation="0.9" flood-color="#bdeeff" flood-opacity="0.65" />
    </filter>
    <filter id={`${id}-light`} filterUnits="userSpaceOnUse" x="-10" y="-10" width="120" height="120">
      <feDropShadow dx="0" dy="0" stdDeviation="1.3" flood-color="#fff" />
      <feDropShadow dx="0" dy="0" stdDeviation="4.2" flood-color="#bdeeff" flood-opacity="0.65" />
    </filter>
  </defs>
  <g fill="none" stroke-linecap="round" stroke-linejoin="round">
    <g filter={`url(#${id}-line)`} stroke="#bdeeff" stroke-width="3.6" opacity="0.6">
      <path d={path} />
    </g>
    <g filter={`url(#${id}-light)`} stroke="#ebfaff" stroke-width="4.2" stroke-linecap="butt" stroke-dashoffset={headOffset}>
      {#each trail as [dasharray, opacity]}
        <path d={path} pathLength="1000" stroke-dasharray={dasharray} {opacity} />
      {/each}
      <path d={path} pathLength="1000" stroke-linecap="round" stroke-dasharray="8.105 1972.554" stroke-dashoffset={headOffset - 316.081} />
    </g>
  </g>
</svg>

<style>
  svg {
    display: block;
    flex: none;
  }
</style>
