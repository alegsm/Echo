<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    title,
    subtitle,
    actions
  }: {
    title: string;
    subtitle?: string;
    actions?: Snippet;
  } = $props();
</script>

<header class="page-header">
  <div class="page-header__row">
    <div class="page-header__title-group">
      <h1 class="page-header__title" tabindex="-1">{title}</h1>
      {#if subtitle}
        <p class="page-header__subtitle">{subtitle}</p>
      {/if}
    </div>

    <div class="page-header__trailing">
      {#if actions}
        <div class="page-header__actions">
          {@render actions()}
        </div>
      {/if}
    </div>
  </div>
</header>

<style>
  .page-header { flex-shrink: 0; width: 100%; padding: 4px 4px 8px; text-shadow: var(--echo-field-halo); }
  .page-header__row { display: flex; align-items: flex-end; justify-content: space-between; gap: 16px; }
  .page-header__title-group { min-width: 0; }
  .page-header__title { margin: 0; font: var(--t-title); letter-spacing: -0.015em; color: var(--echo-fg); }
  .page-header__subtitle { margin: 8px 0 0; font: var(--t-meta); color: var(--echo-fg-2); }
  .page-header__trailing, .page-header__actions { display: flex; align-items: center; gap: 8px; }
  .page-header__trailing { flex-shrink: 0; max-width: 100%; }
  @media (max-width: 767px) {
    .page-header__row { flex-wrap: wrap; }
    .page-header__trailing, .page-header__actions { flex-wrap: wrap; }
  }
</style>
