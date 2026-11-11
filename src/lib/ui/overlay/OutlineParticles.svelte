<script lang="ts">
  import { roundedRectPerimeter, sweepHeadDistance, sweepLightPieces } from './outline-particles';

  const filterId = $props.id();
  const perimeter = roundedRectPerimeter(330, 46, 23);
  const lightPieces = sweepLightPieces(perimeter, 128);
  const bloomPieces = sweepLightPieces(perimeter, 32);

  let { eligible = false, reducedMotion = false, warning = false }: { eligible?: boolean; reducedMotion?: boolean; warning?: boolean } = $props();

  function animateOutline(node: SVGSVGElement) {
    const canAnimate = eligible && !reducedMotion;
    let frame: number | undefined;
    let active = true;
    function paint(now: number) {
      const distance = sweepHeadDistance(now, perimeter);
      node.style.setProperty('--sweep-head', `${distance}`);
    }
    function tick(now: number) {
      if (!active) return;
      paint(now);
      frame = requestAnimationFrame(tick);
    }
    paint(performance.now());
    if (canAnimate) frame = requestAnimationFrame(tick);
    return () => {
      active = false;
      if (frame !== undefined) cancelAnimationFrame(frame);
    };
  }
</script>

{#snippet tail(pieces: typeof lightPieces)}
  {#each pieces as piece, index (index)}
    <rect
      x="1" y="1" width="330" height="46" rx="23" pathLength={perimeter}
      stroke-dasharray={`${piece.length} ${perimeter - piece.length}`}
      style:stroke-dashoffset={`calc(${piece.length} - var(--sweep-head, 0))`}
      opacity={piece.opacity}
    />
  {/each}
{/snippet}

<div class="outline-particles">
  {#if warning}
    <svg class="outline-drawing outline-warning-base" viewBox="0 0 332 48" preserveAspectRatio="none" aria-hidden="true">
      <rect class="outline-line" x="1" y="1" width="330" height="46" rx="23" />
    </svg>
  {/if}
  <div class="outline-layers" class:warning-light={warning}>
    <!-- Keep the static filtered line in its own SVG so sweep paints never refilter it. -->
    <svg class="outline-drawing outline-static" viewBox="0 0 332 48" preserveAspectRatio="none" aria-hidden="true">
      <defs>
        <filter id={`${filterId}-glow`} filterUnits="userSpaceOnUse" x="-12" y="-12" width="356" height="72">
          <feDropShadow dx="0" dy="0" stdDeviation="1" style="flood-color: var(--glass-glow, rgba(189, 238, 255, 0.65));" />
        </filter>
      </defs>
      <rect class="outline-line" filter={`url(#${filterId}-glow)`} x="1" y="1" width="330" height="46" rx="23" pathLength={perimeter} />
    </svg>
    <svg data-outline-particles class="outline-drawing outline-sweep" viewBox="0 0 332 48" preserveAspectRatio="none" aria-hidden="true" {@attach animateOutline}>
      <g class="outline-light">
        <!-- Low-frequency under-layers share the core's head and linear taper. -->
        <g class="outline-tail-bloom" stroke-width="8" opacity="0.08">{@render tail(bloomPieces)}</g>
        <g class="outline-tail-bloom" stroke-width="4" opacity="0.24">{@render tail(bloomPieces)}</g>
        <g class="outline-tail-core" stroke-width="2">{@render tail(lightPieces)}</g>
      </g>
    </svg>
  </div>
</div>
