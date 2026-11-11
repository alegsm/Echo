const SWEEP_LAP_MS = 2400;
const SWEEP_CORE_PX = 80;
const SWEEP_FADE_MS = 400;

export function roundedRectPerimeter(width: number, height: number, radius: number): number {
  return 2 * (width + height - 4 * radius) + 2 * Math.PI * radius;
}

/** Distances use the page's monotonic clock so remounts resume the same lap. */
export function sweepHeadDistance(elapsedMs: number, perimeter: number): number {
  return (elapsedMs % SWEEP_LAP_MS) / SWEEP_LAP_MS * perimeter;
}

function sweepTailLength(perimeter: number): number {
  return SWEEP_CORE_PX + perimeter * SWEEP_FADE_MS / SWEEP_LAP_MS;
}

/** Nested dashes share their head and combine into a linear opacity fade. */
export function sweepLightPieces(perimeter: number, count: number): Array<{ length: number; opacity: number }> {
  const step = sweepTailLength(perimeter) / count;
  return Array.from({ length: count }, (_, index) => ({
    length: (index + 1) * step,
    // From layer k onward, transmission telescopes to k/count.
    opacity: 1 / (index + 1)
  }));
}
