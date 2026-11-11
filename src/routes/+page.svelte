<script lang="ts">
  import { onMount } from 'svelte';
  import type { Component } from 'svelte';
  import { attachTauriFileLogging, detachTauriFileLogging } from '#lib/runtime/tauri-log.ts';

  let SurfaceComponent: Component | undefined = $state();

  onMount(() => {
    void attachTauriFileLogging();
    void mountSurface();

    return () => {
      detachTauriFileLogging();
    };
  });

  async function mountSurface() {
    const surface = new URLSearchParams(window.location.search).get('surface');
    SurfaceComponent =
      surface === 'overlay'
        ? (await import('#lib/ui/overlay/OverlayApp.svelte')).default
        : (await import('#lib/ui/dashboard/DashboardApp.svelte')).default;
  }
</script>

{#if SurfaceComponent}
  <SurfaceComponent />
{/if}
