<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    padding = '0',
    children
  }: {
    padding?: string;
    children: Snippet;
  } = $props();
</script>

<div class="dashboard-card" style:padding>
  <div class="dashboard-card__inner">
    {@render children()}
  </div>
</div>

<style>
  .dashboard-card {
    display: flex;
    flex-direction: column;
    width: 100%;
    border-radius: var(--r-card);
    background: var(--echo-card);
    box-shadow: inset 0 0 0 1px var(--echo-hairline);
    -webkit-backdrop-filter: var(--echo-frost);
    backdrop-filter: var(--echo-frost);
  }
  .dashboard-card__inner { display: flex; flex-direction: column; flex: 1 1 auto; min-width: 0; }
</style>
