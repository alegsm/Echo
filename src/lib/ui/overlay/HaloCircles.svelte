<script lang="ts">
  // Ported from an MIT-licensed particle halo component. See ParticleHalo.LICENSE for the upstream notice.

  let { circles, nodes = [] }: { circles: { x: number; y: number; r: number; color: string }[]; nodes?: SVGCircleElement[] } = $props();
</script>

{#each circles as point, i (i)}
  <circle bind:this={nodes[i]} cx={point.x} cy={point.y} r={point.r} fill={point.color} />
{/each}
