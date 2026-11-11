<script lang="ts">
  import { observeDashboardMotion, observeDashboardIntersection } from '../dashboard-motion';
  import { roundedRectPerimeter, sweepHeadDistance, sweepLightPieces } from '../../overlay/outline-particles';

  const filterId = $props.id();
  let width = $state(120);
  let height = $state(34);
  const rectWidth = $derived(Math.max(0, width - 2));
  const rectHeight = $derived(Math.max(0, height - 2));
  const radius = $derived(Math.min(rectWidth, rectHeight) / 2);
  const perimeter = $derived(roundedRectPerimeter(rectWidth, rectHeight, radius));
  const lightPieces = $derived(sweepLightPieces(perimeter, 128));

  function animateOutline(node: SVGSVGElement) {
    let active = false;
    let visible = false;
    let frame = 0;
    function paint(now: number) {
      node.style.setProperty('--sweep-head', `${sweepHeadDistance(now, perimeter)}`);
    }
    function tick(now: number) {
      paint(now);
      frame = requestAnimationFrame(tick);
    }
    function syncMotion() {
      cancelAnimationFrame(frame);
      frame = 0;
      if (!active || !visible) return;
      paint(performance.now());
      frame = requestAnimationFrame(tick);
    }
    const stopMotion = observeDashboardMotion({ window, document }, (motion) => {
      active = motion.active;
      syncMotion();
    });
    const stopIntersection = observeDashboardIntersection(node, (intersecting) => {
      visible = intersecting;
      syncMotion();
    });
    return () => {
      cancelAnimationFrame(frame);
      stopMotion();
      stopIntersection();
    };
  }
</script>

<span class="check-light" bind:clientWidth={width} bind:clientHeight={height} aria-hidden="true">
  <svg viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" {@attach animateOutline}>
    <defs>
      <filter id={`${filterId}-line`} filterUnits="userSpaceOnUse" x="-24" y="-24" width={width + 48} height={height + 48}>
        <feDropShadow dx="0" dy="0" stdDeviation="1" style="flood-color: color-mix(in srgb, var(--tone-loading) 65%, transparent);" />
      </filter>
      <filter id={`${filterId}-light`} filterUnits="userSpaceOnUse" x="-24" y="-24" width={width + 48} height={height + 48}>
        <feDropShadow dx="0" dy="0" stdDeviation="1.5" style="flood-color: #fff;" />
        <feDropShadow dx="0" dy="0" stdDeviation="5" style="flood-color: color-mix(in srgb, var(--tone-loading) 65%, transparent);" />
      </filter>
    </defs>
    <rect class="outline-line" filter={`url(#${filterId}-line)`} x="1" y="1" width={rectWidth} height={rectHeight} rx={radius} />
    <g class="outline-light" filter={`url(#${filterId}-light)`}>
      {#each lightPieces as piece, index (index)}
        <rect x="1" y="1" width={rectWidth} height={rectHeight} rx={radius} pathLength={perimeter}
          stroke-dasharray={`${piece.length} ${perimeter - piece.length}`}
          style:stroke-dashoffset={`calc(${piece.length} - var(--sweep-head, 0))`} opacity={piece.opacity} />
      {/each}
    </g>
  </svg>
</span>

<style>
  .check-light { position: absolute; inset: 0; pointer-events: none; }
  svg { display: block; width: 100%; height: 100%; overflow: visible; fill: none; }
  .outline-line { stroke: var(--tone-loading); stroke-width: 1.1px; opacity: 0.55; }
  .outline-light { stroke: color-mix(in srgb, var(--tone-loading), white 70%); stroke-width: 2px; }
  @media (prefers-reduced-motion: reduce) { .check-light { display: none; } }
</style>
