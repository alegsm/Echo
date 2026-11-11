<script lang="ts">
  import { onMount } from 'svelte';
  import { mountParticleField } from './particle-field';

  let canvas: HTMLCanvasElement;

  onMount(() => mountParticleField(canvas, {
    window, document, now: () => performance.now(),
    requestFrame: (callback) => requestAnimationFrame(callback), cancelFrame: (id) => cancelAnimationFrame(id)
  }));
</script>

<canvas bind:this={canvas} aria-hidden="true"></canvas>

<style>
  canvas {
    position: fixed;
    inset: 0;
    width: 100%;
    height: 100%;
    z-index: 0;
    opacity: var(--echo-field-opacity, 1);
    pointer-events: none;
  }
</style>
