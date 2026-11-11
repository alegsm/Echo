// Ported from an MIT-licensed particle halo component. See ParticleHalo.LICENSE for the upstream notice.
import type { TonePose } from './state-tone';

export type ParticlePoint = { x: number; y: number };

const THREAD_CONFIG = Array.from({ length: 6 }, (_, i) => ({
  baseRadius: 28,
  radiusSpread: (i % 4) * 1.8 - 2.7,
  ryOffset: ((i % 3) - 1) * 0.8,
  startDeg: i * 60 + Math.sin(i * 2.3) * 12,
  baseFreq: 5 + (i % 4),
  baseAmp: 3 + Math.sin(i * 1.7) * 2,
  phaseOffset: i * 0.9,
  ampModPeriod: [7.3, 5.7, 8.9, 6.1, 9.7, 4.3][i],
  ampModDepth: [5.0, 6.0, 4.0, 7.0, 5.6, 4.4][i],
  freqModPeriod: [11.3, 9.1, 13.7, 8.3, 10.9, 12.1][i],
  freqModDepth: [1.6, 2.0, 1.2, 1.8, 1.4, 2.2][i]
}));

function sampleThreadPoint(index: number, t: number, time: number, out: ParticlePoint): ParticlePoint {
  const config = THREAD_CONFIG[index];
  const wobbleAmp = config.baseAmp +
    Math.sin((time / config.ampModPeriod) * Math.PI * 2) * config.ampModDepth;
  const rawFreq = config.baseFreq +
    Math.sin((time / config.freqModPeriod) * Math.PI * 2) * config.freqModDepth;
  const freqLo = Math.floor(rawFreq);
  const freqHi = freqLo + 1;
  const freqFrac = rawFreq - freqLo;
  const freqBlend = freqFrac * freqFrac * (3 - 2 * freqFrac);
  const baseRx = config.baseRadius + config.radiusSpread;
  const baseRy = baseRx + config.ryOffset;
  const angleRad = ((config.startDeg + 360 * t) * Math.PI) / 180;
  const wobbleLo = Math.sin(freqLo * t * Math.PI * 2 + config.phaseOffset) * wobbleAmp;
  const wobbleHi = Math.sin(freqHi * t * Math.PI * 2 + config.phaseOffset) * wobbleAmp;
  const wobble = wobbleLo + (wobbleHi - wobbleLo) * freqBlend;
  out.x = 50 + (baseRx + wobble) * Math.cos(angleRad);
  out.y = 50 + (baseRy + wobble * 0.7) * Math.sin(angleRad);
  return out;
}

/** Particle coordinates in the 100×100 SVG viewBox; time is integrated geometry seconds. */
export function particleTarget(
  pose: TonePose,
  i: number,
  count: number,
  time: number,
  haloMode: 'live' | 'frozen' = 'live',
  reducedMotion = false,
  out: ParticlePoint = { x: 0, y: 0 }
): ParticlePoint {
  const thread = i % 6;
  const slot = Math.floor(i / 6);
  if (pose === 'rest') {
    out.x = 10 + (i / (count - 1)) * 80;
    out.y = 50;
    return out;
  }
  const perThread = count / 6;
  if (pose === 'halo') {
    const t = perThread > 1 ? slot / (perThread - 1) : 0;
    return sampleThreadPoint(thread, t, reducedMotion || haloMode === 'frozen' ? 0 : time, out);
  }
  const arc = (Math.PI * 2) / 6;
  const angle = thread * arc + ((slot + 0.5) / perThread) * arc +
    time * (reducedMotion ? 0 : 0.08) * Math.PI * 2;
  out.x = 50 + 28 * Math.cos(angle);
  out.y = 50 + 28 * Math.sin(angle);
  return out;
}

export type HaloMotionProps = {
  eligible?: boolean;
  colors: string[];
  pose: TonePose;
  haloMode?: 'live' | 'frozen';
  particleCount: number;
  animationSpeed: number;
  transitionMs: number;
};

/** Owns one interruptible loop. Call update after input or eligibility changes. */
export function mountParticleHalo(
  getProps: () => HaloMotionProps,
  publish: (points: ParticlePoint[], frameColors: string[]) => void,
  effects: {
    reducedMotion: boolean | (() => boolean);
    requestFrame: (tick: (now: number) => void) => number;
    cancelFrame: (frame: number) => void;
  }
) {
  const initial = getProps();
  const count = Math.max(6, Math.round(initial.particleCount / 6) * 6);
  const particles = Array.from({ length: count }, (_, i) => particleTarget('rest', i, count, 0));
  const origins = particles.map(point => ({ ...point }));
  const target = { x: 0, y: 0 };
  const frameColors = initial.colors.slice();
  let lastPose = initial.pose;
  let tweenStart: number | undefined;
  let tweenDuration = 0;
  let geometryTime = 0;
  let lastFrameTime: number | undefined;
  let frame: number | undefined;
  let generation = 0;
  let disposed = false;
  let transitioning = false;
  const reduced = () => typeof effects.reducedMotion === 'function' ? effects.reducedMotion() : effects.reducedMotion;
  const moving = (props: HaloMotionProps) => props.pose !== 'rest' && !(props.pose === 'halo' && props.haloMode === 'frozen');

  function paint(now: number, settle = false) {
    const props = getProps();
    for (let i = 0; i < frameColors.length; i++) frameColors[i] = props.colors[i];
    if (props.pose !== lastPose) {
      for (let i = 0; i < count; i++) {
        origins[i].x = particles[i].x;
        origins[i].y = particles[i].y;
      }
      tweenStart = now;
      tweenDuration = props.transitionMs;
      transitioning = tweenDuration > 0;
      lastPose = props.pose;
    }
    const progress = settle || !transitioning ? 1 : Math.min((now - tweenStart!) / tweenDuration, 1);
    const eased = 1 - Math.pow(1 - progress, 3);
    for (let i = 0; i < count; i++) {
      particleTarget(props.pose, i, count, geometryTime, props.haloMode, reduced(), target);
      particles[i].x = progress === 1 ? target.x : origins[i].x + (target.x - origins[i].x) * eased;
      particles[i].y = progress === 1 ? target.y : origins[i].y + (target.y - origins[i].y) * eased;
    }
    if (progress === 1) transitioning = false;
    publish(particles, frameColors);
  }

  function stop() {
    generation++;
    if (frame !== undefined) effects.cancelFrame(frame);
    frame = undefined;
    lastFrameTime = undefined;
  }

  function update() {
    if (disposed) return;
    const props = getProps();
    if (props.eligible === false || reduced()) {
      stop();
      paint(0, true);
      return;
    }
    if (frame !== undefined) return;
    if (props.pose === lastPose && !moving(props) && !transitioning) {
      paint(0, true);
      return;
    }
    const currentGeneration = ++generation;
    // This closure is created only when a loop starts, never by a frame.
    const tick = (now: number) => {
      if (disposed || currentGeneration !== generation) return;
      frame = undefined;
      const props = getProps();
      if (props.eligible === false || reduced()) { update(); return; }
      if (lastFrameTime !== undefined) geometryTime += ((now - lastFrameTime) / 1000) * props.animationSpeed;
      lastFrameTime = now;
      paint(now);
      if (moving(props) || transitioning) frame = effects.requestFrame(tick);
      else lastFrameTime = undefined;
    };
    frame = effects.requestFrame(tick);
  }

  publish(particles, frameColors);
  update();
  function dispose() { stop(); disposed = true; }
  return Object.assign(dispose, { update, stop, dispose });
}
