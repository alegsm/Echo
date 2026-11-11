<script lang="ts">
  import { getDashboardContext } from './context';
  import { desktopPlatform } from './panels/permissions-controller';
  import { getCurrentWindow } from '@tauri-apps/api/window';

  const ctx = getDashboardContext();
  const platform = $derived(desktopPlatform(ctx.permissions()));
  const isWindows = $derived(platform === 'windows');
  const isMac = $derived(platform === 'macos');
  let isMaximized = $state(false);

  $effect(() => {
    if (!isWindows) return;

    const win = getCurrentWindow();
    let unlisten: (() => void) | undefined;
    let disposed = false;

    (async () => {
      const maximized = await win.isMaximized();
      if (disposed) return;
      isMaximized = maximized;
      const stop = await win.onResized(async () => {
        const maximized = await win.isMaximized();
        if (!disposed) isMaximized = maximized;
      });
      if (disposed) stop();
      else unlisten = stop;
    })();

    return () => { disposed = true; unlisten?.(); };
  });

  async function minimize() {
    await getCurrentWindow().minimize();
  }
  async function toggleMaximize() {
    await getCurrentWindow().toggleMaximize();
  }
  async function close() {
    await getCurrentWindow().close();
  }
</script>

{#if isWindows}
  <div class="titlebar">
    <div class="titlebar-spacer" data-tauri-drag-region></div>
    <div class="titlebar-controls">
      <button class="ctl" onclick={minimize} aria-label="Minimize" type="button">
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <rect x="1" y="5" width="8" height="1" fill="currentColor" />
        </svg>
      </button>
      <button
        class="ctl"
        onclick={toggleMaximize}
        aria-label={isMaximized ? 'Restore' : 'Maximize'}
        type="button"
      >
        {#if isMaximized}
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
            <rect x="0.5" y="2.5" width="7" height="7" stroke="currentColor" />
            <path d="M2.5 2.5V0.5H9.5V7.5H7.5" stroke="currentColor" />
          </svg>
        {:else}
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
            <rect x="0.5" y="0.5" width="9" height="9" stroke="currentColor" />
          </svg>
        {/if}
      </button>
      <button class="ctl ctl-close" onclick={close} aria-label="Close" type="button">
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
          <path d="M1 1L9 9M9 1L1 9" stroke="currentColor" stroke-width="1" />
        </svg>
      </button>
    </div>
  </div>
{:else if isMac}
  <!-- Macos: titleBarStyle "Overlay" + hiddenTitle keep the traffic lights on top of
       the content area; this absolute strip gives the user 22px of empty drag surface
       to grab around them without competing with clickable UI below. -->
  <div class="mac-drag-strip" data-tauri-drag-region aria-hidden="true"></div>
{/if}

<style>
  /* The bar owns its placement: absolute over the shell's reserved
     --dashboard-chrome-top band, outside the dashboard grid flow. */
  .titlebar {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 32px;
    display: flex;
    align-items: stretch;
    background: transparent;
    user-select: none;
    -webkit-user-select: none;
    z-index: 10;
  }

  .titlebar-spacer {
    flex: 1;
  }

  .titlebar-controls {
    display: flex;
    height: 100%;
  }

  .ctl {
    width: 46px;
    height: 100%;
    background: transparent;
    border: none;
    padding: 0;
    color: var(--echo-chalk-fg);
    cursor: default;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: background 80ms ease;
  }

  .ctl:hover {
    background: rgba(255, 255, 255, 0.08);
  }

  .ctl:active {
    background: rgba(255, 255, 255, 0.04);
  }

  .ctl-close:hover {
    background: #c42b1c;
    color: #ffffff;
  }

  .ctl-close:active {
    background: #b8271a;
  }

  .mac-drag-strip {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 22px;
    z-index: 10;
    pointer-events: auto;
  }
</style>
