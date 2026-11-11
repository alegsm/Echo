import { observeDashboardMotion, publishDashboardFrame, type DashboardMotion, type DashboardMotionEnvironment } from './dashboard-motion';

type ParticleEnvironment = DashboardMotionEnvironment & {
  window: Pick<Window, 'matchMedia' | 'devicePixelRatio' | 'innerWidth' | 'innerHeight' | 'addEventListener' | 'removeEventListener'>;
  now: () => number;
  requestFrame: (callback: FrameRequestCallback) => number;
  cancelFrame: (id: number) => void;
};

export function mountParticleField(canvas: HTMLCanvasElement, environment: ParticleEnvironment) {
  const context = canvas.getContext('2d');
  if (!context) return;

  // Port of the approved B v2 canvas: 180 × 120 points, wave mode, orbital camera.
  const gridWidth = 180;
  const gridHeight = 120;
  const count = gridWidth * gridHeight;
  const baseX = new Float32Array(count);
  const baseY = new Float32Array(count);
  for (let x = 0, point = 0; x < gridWidth; x++) {
    for (let y = 0; y < gridHeight; y++, point++) {
      baseX[point] = (x - gridWidth / 2) * 1.2;
      baseY[point] = (y - gridHeight / 2) * 1.2;
    }
  }
  const levels = 24;
  const buckets = Array.from({ length: levels }, () => new Float32Array(count * 3));
  const counts = new Int32Array(levels);
  const focalLength = 1 / Math.tan(30 * Math.PI / 180);
  const columnWave = new Float64Array(gridWidth);
  const rowWave = new Float64Array(gridHeight);
  let width = 0;
  let height = 0;
  let backingWidth = 0;
  let backingHeight = 0;
  let pointSize = 125;
  let frame = 0;
  let active = false;
  let reduced = false;
  let elapsed = 0;
  let resumedAt = 0;
  let lastDraw = 0;
  let dirty = true;
  let initialized = false;
  const frameInterval = 1000 / 30;

  function draw(seconds: number) {
    // Camera looks at (20, 0, 0), with the same up vector and 60° field of view.
    const time = seconds * 1.5;
    const eyeX = 20 + Math.sin(seconds * 0.2) * 5;
    const eyeY = Math.cos(seconds * 0.15) * 5;
    const eyeZ = 80;
    let zx = eyeX - 20, zy = eyeY, zz = eyeZ;
    const zLength = Math.hypot(zx, zy, zz);
    zx /= zLength; zy /= zLength; zz /= zLength;
    let xx = zz, xz = -zx;
    const xLength = Math.hypot(xx, xz);
    xx /= xLength; xz /= xLength;
    const yx = zy * xz, yy = zz * xx - zx * xz, yz = -zy * xx;
    const aspect = width / height;
    counts.fill(0);
    // Read the original Float32 coordinates before evaluating each wave in double precision.
    for (let x = 0; x < gridWidth; x++) columnWave[x] = Math.sin(baseX[x * gridHeight] * 0.1 + time) * 6;
    for (let y = 0; y < gridHeight; y++) rowWave[y] = Math.cos(baseY[y] * 0.1 + time) * 4;
    for (let column = 0, point = 0; column < gridWidth; column++) {
      const wave = columnWave[column];
      for (let row = 0; row < gridHeight; row++, point++) {
        const x = baseX[point], y = baseY[point];
        const z = wave + rowWave[row];
        const dx = x - eyeX, dy = y - eyeY, dz = z - eyeZ;
        const viewX = dx * xx + dz * xz;
        const viewY = dx * yx + dy * yy + dz * yz;
        const viewZ = dx * zx + dy * zy + dz * zz;
        if (viewZ >= -0.1) continue;
        const inverseDepth = 1 / -viewZ;
        const screenX = (viewX * focalLength / aspect * inverseDepth + 1) * 0.5 * backingWidth;
        const screenY = (1 - viewY * focalLength * inverseDepth) * 0.5 * backingHeight;
        const size = pointSize * inverseDepth;
        if (screenX < -size || screenX > backingWidth + size || screenY < -size || screenY > backingHeight + size) continue;
        const depth = Math.min(1, Math.max(0, (z + 10) / 20));
        const alpha = depth * depth * (3 - 2 * depth);
        const level = Math.min(levels - 1, Math.floor(alpha * levels));
        const bucket = buckets[level];
        const offset = counts[level]++ * 3;
        bucket[offset] = screenX;
        bucket[offset + 1] = screenY;
        bucket[offset + 2] = size;
      }
    }
    context!.clearRect(0, 0, backingWidth, backingHeight);
    context!.fillStyle = '#c8c2bc';
    for (let level = 0; level < levels; level++) {
      if (!counts[level]) continue;
      context!.globalAlpha = 0.1 + 0.6 * ((level + 0.5) / levels);
      context!.beginPath();
      const bucket = buckets[level];
      for (let point = 0; point < counts[level]; point++) {
        const offset = point * 3;
        // Match the reference's measured coverage of aliased GL points.
        const size = bucket[offset + 2] * 0.72;
        context!.rect(bucket[offset] - size / 2, bucket[offset + 1] - size / 2, size, size);
      }
      context!.fill();
    }
    context!.globalAlpha = 1;
  }

  function render(seconds: number) {
    if (dirty) {
      const dpr = Math.min(environment.window.devicePixelRatio || 1, 2);
      const scale = Math.max(1, dpr * 0.5);
      width = environment.window.innerWidth;
      height = environment.window.innerHeight;
      backingWidth = Math.round(width * scale);
      backingHeight = Math.round(height * scale);
      pointSize = 250 * scale / dpr;
      canvas.width = backingWidth;
      canvas.height = backingHeight;
      dirty = false;
    }
    draw(seconds);
    if (active) publishDashboardFrame(environment.document, environment.now(), seconds * 1000);
  }

  function resize() {
    dirty = true;
    if (active) render((elapsed + environment.now() - resumedAt) / 1000);
    else if (reduced) render(0);
  }

  function tick(now: number) {
    frame = 0;
    const sinceDraw = now - lastDraw;
    if (sinceDraw + 0.001 >= frameInterval) {
      render((elapsed + now - resumedAt) / 1000);
      // Preserve the cadence at high refresh rates without drawing missed frames.
      lastDraw = now - (sinceDraw + 0.001) % frameInterval;
    }
    frame = environment.requestFrame(tick);
  }

  function sync(motion: DashboardMotion) {
    const now = environment.now();
    if (active) elapsed += now - resumedAt;
    const reset = motion.reduced && !reduced;
    active = motion.active;
    reduced = motion.reduced;
    environment.cancelFrame(frame);
    frame = 0;
    if (reduced) elapsed = 0;
    resumedAt = now;
    lastDraw = now;
    if (!initialized || reset || (active && dirty)) render(elapsed / 1000);
    initialized = true;
    if (active) frame = environment.requestFrame(tick);
  }

  const stopMotion = observeDashboardMotion(environment, sync);
  environment.window.addEventListener('resize', resize);
  return () => {
    environment.cancelFrame(frame);
    stopMotion();
    environment.window.removeEventListener('resize', resize);
  };
}
