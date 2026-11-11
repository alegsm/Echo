/** Palette, animation speed and ParticleHalo pose for each overlay tone. */
export type StatusTone = 'idle' | 'active' | 'handsFree' | 'warning' | 'loading' | 'success' | 'error';

/** ParticleHalo pose values. Mirrors the upstream `Pose` union. */
export type TonePose = 'rest' | 'halo' | 'ring';
/** ParticleHalo halo mode used by the overlay. */
export type ToneHaloMode = 'live';

/** Halo color for each tone. */
export const TONE_COLOR: Record<StatusTone, string> = {
  idle: '#f7f4ef',
  active: '#bdeeff',
  handsFree: '#ffd18a',
  warning: '#ff5b64',
  loading: '#c9b8ff',
  success: '#47f08a',
  error: '#ff5a2d',
};

/** Halo `animationSpeed` per tone — matches `orbMotionSpeed` in OverlayApp. */
export const TONE_SPEED: Record<StatusTone, number> = {
  idle: 1,
  active: 3,
  handsFree: 3,
  warning: 3,
  loading: 3,
  success: 1,
  error: 1,
};

const TONE_POSE: Record<StatusTone, { pose: TonePose; haloMode?: ToneHaloMode }> = {
  idle: { pose: 'rest' },
  active: { pose: 'halo', haloMode: 'live' },
  handsFree: { pose: 'halo', haloMode: 'live' },
  warning: { pose: 'halo', haloMode: 'live' },
  loading: { pose: 'ring' },
  success: { pose: 'halo', haloMode: 'live' },
  error: { pose: 'halo', haloMode: 'live' },
};

export function resolveTonePose(tone: StatusTone): {
  pose: TonePose;
  haloMode?: ToneHaloMode;
} {
  return { ...TONE_POSE[tone] };
}
