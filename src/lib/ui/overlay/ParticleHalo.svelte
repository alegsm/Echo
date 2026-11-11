<script lang="ts">
  // Ported from an MIT-licensed particle halo component. See ParticleHalo.LICENSE for the upstream notice.
  import HaloCircles from './HaloCircles.svelte';
  import { onMount, untrack } from 'svelte';
  import { mountParticleHalo, particleTarget, type HaloMotionProps } from './particle-halo-geometry';
  import type { ToneHaloMode, TonePose } from './state-tone';

  type Props = {
    class?: string;
    eligible?: boolean;
    warning?: boolean;
    reducedMotion?: boolean;
    motion?: { colors: string[]; speed: number };
    colors: string[];
    pose: TonePose;
    haloMode?: ToneHaloMode;
    particleCount: number;
    animationSpeed: number;
    transitionMs: number;
  };

  let {
    class: className = '', colors, pose, haloMode = 'live',
    particleCount, animationSpeed, transitionMs, eligible = true, reducedMotion = false, warning = false, motion
  }: Props = $props();
  const count = Math.max(6, Math.round(untrack(() => particleCount) / 6) * 6);
  const circles = Array.from({ length: count }, (_, i) => ({
    ...particleTarget('rest', i, count, 0), r: 0.9, color: colors[i % 6]
  }));
  const nodes: SVGCircleElement[] = [];
  const baseNodes: SVGCircleElement[] = [];
  const fills = new WeakMap<SVGCircleElement, string>();
  let controller: ReturnType<typeof mountParticleHalo> | undefined;
  const motionProps: HaloMotionProps = {
    get pose() { return pose; }, get haloMode() { return haloMode; },
    get particleCount() { return particleCount; }, get transitionMs() { return transitionMs; },
    get animationSpeed() { return motion?.speed ?? animationSpeed; },
    // Reduced-motion paints use the target palette without waiting for the parent's motion sample.
    get colors() { return reducedMotion ? colors : motion?.colors ?? colors; }, get eligible() { return eligible; }
  };

  onMount(() => {
    controller = mountParticleHalo(() => motionProps, (points, sampledColors) => {
      for (let i = 0; i < nodes.length; i++) {
        const node = nodes[i];
        const x = String(points[i].x), y = String(points[i].y);
        node.setAttribute('cx', x);
        node.setAttribute('cy', y);
        if (warning) {
          baseNodes[i].setAttribute('cx', x);
          baseNodes[i].setAttribute('cy', y);
        }
        const fill = sampledColors[i % 6];
        if (fills.get(node) !== fill) { node.setAttribute('fill', fill); fills.set(node, fill); }
      }
    }, {
      reducedMotion: () => reducedMotion,
      requestFrame: (tick) => requestAnimationFrame(tick),
      cancelFrame: (frame) => cancelAnimationFrame(frame)
    });
    return () => { controller?.dispose(); controller = undefined; };
  });
  $effect(() => {
    // Input changes wake a settled loop; frame samples never publish reactive points.
    pose; haloMode; transitionMs; animationSpeed; colors; eligible; reducedMotion; warning; motion;
    controller?.update();
  });
</script>

{#snippet drawing()}
  <svg viewBox="0 0 100 100" width="120" height="120" fill="none" class={className} aria-hidden="true">
    <HaloCircles {circles} {nodes} />
  </svg>
{/snippet}

{#if warning}
  <svg class="warning-base" viewBox="0 0 100 100" width="120" height="120" fill="none" aria-hidden="true">
    <HaloCircles {circles} nodes={baseNodes} />
  </svg>
  <span class="warning-light">{@render drawing()}</span>
{:else}
  {@render drawing()}
{/if}
