<script lang="ts">
  let { message, label = 'Network Failure' }: { message: string; label?: string } = $props();
</script>

<div class="alert-banner alert-banner--error" role="alert" aria-live="assertive">
  <div class="alert-banner__icon">X</div>
  <div class="alert-banner__content">
    <div class="alert-banner__label">{label}</div>
    <div class="alert-banner__message">{message}</div>
  </div>
</div>

<style>
  .alert-banner {
    display: flex;
    align-items: center;
    gap: var(--echo-alert-space);
    padding: var(--echo-alert-space);
    background: var(--echo-alert-fill);
    -webkit-backdrop-filter: blur(var(--echo-alert-blur)) saturate(var(--echo-alert-saturate));
    backdrop-filter: blur(var(--echo-alert-blur)) saturate(var(--echo-alert-saturate));
    border: 1px solid var(--echo-chalk-border);
    border-left-width: 4px;
    border-left-color: var(--echo-alert-error);
  }
  .alert-banner__icon {
    width: 16px;
    height: 16px;
    border-radius: var(--echo-small-radius);
    display: flex;
    align-items: center;
    justify-content: center;
    font-family: var(--echo-chalk-mono);
    font-size: 8px;
    flex-shrink: 0;
    border: 1px solid var(--echo-alert-error);
    color: var(--echo-alert-error);
  }
  .alert-banner__label {
    font-family: var(--echo-chalk-mono);
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.15em;
    color: var(--echo-alert-error);
  }
  .alert-banner__message {
    font-family: var(--echo-chalk-mono);
    font-size: 11px;
    color: var(--echo-chalk-fg);
  }
</style>
