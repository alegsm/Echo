<script lang="ts">
  import { onMount } from 'svelte';
  import './overlay-shell.css';
  import './overlay-details.css';
  import OverlaySurface from './OverlaySurface.svelte';
  import { TONE_SPEED, TONE_COLOR, resolveTonePose } from './state-tone';
  import { createOverlayWindowController } from './overlay-window.svelte';
  import { reduceShell, retainsPreviewWindow } from './overlay-shell-phase';
  import { createOverlayController, initialOverlayState, overlayPresentation, errorLabels } from './overlay-controller';
  import { providerOptions } from '#lib/core/settings.ts';
  import { createEchoRuntime } from '#lib/runtime/runtime.ts';
  import type { EchoRuntime } from '#lib/core/types.ts';

  let { createRuntime = createEchoRuntime }: { createRuntime?: typeof createEchoRuntime } = $props();

  let reducedMotion = $state(false);
  let documentVisible = $state(true);
  const motion = { speed: 1, colors: Array<string>(6).fill('#f7f4ef') };
  const colorFrom = new Float64Array(3);
  const colorTarget = new Float64Array(3);
  function readColor(hex: string, out: Float64Array) {
    const value = parseInt(hex.slice(1), 16);
    out[0] = (value >> 16) & 255; out[1] = (value >> 8) & 255; out[2] = value & 255;
  }
  function colorSample(t: number) {
    let color = '#';
    for (let i = 0; i < 3; i++) color += Math.round(colorFrom[i] + (colorTarget[i] - colorFrom[i]) * t).toString(16).padStart(2, '0');
    for (let i = 0; i < 6; i++) motion.colors[i] = color;
  }

  const TERMINAL_STATE_MS = 4000;

  let runtime: EchoRuntime | undefined = $state();
  const viewState = $state(initialOverlayState());
  let terminalGeneration = 0;
  let terminalStateTimer: ReturnType<typeof setTimeout> | undefined;
  const overlayWindow = createOverlayWindowController({
    onError: (message) => {
      viewState.actionError = message;
      showTerminalStateTemporarily();
    }
  });
  const expanded = $derived(overlayWindow.expanded);

  const presentation = $derived(overlayPresentation(viewState));
  const retainedPreview = $derived(Boolean(presentation.warningText) || Boolean(viewState.transcriptionNotice) || retainsPreviewWindow(viewState.shell, Boolean(viewState.preview.text)));
  const isBusy = $derived(presentation.isBusy);
  const isLoading = $derived(presentation.isLoading);
  const hasVisibleError = $derived(presentation.hasVisibleError);
  const hasVisibleSuccess = $derived(presentation.hasVisibleSuccess);
  const statusTone = $derived(presentation.statusTone);
  const activeProvider = $derived(providerOptions.find((provider) => provider.id === viewState.snapshot.activeProviderId));
  const currentModelText = $derived(
    activeProvider ? `${activeProvider.label} / ${activeProvider.model}` : viewState.snapshot.activeProviderId
  );
  const activityText = $derived(presentation.activityText);
  const windowRadius = $derived(expanded ? '28px' : retainedPreview ? '0px' : '999px');
  const orbMotionSpeed = $derived(TONE_SPEED[statusTone]);
  const orbRingColor = $derived(TONE_COLOR[statusTone]);
  const orbRingColors = $derived(Array<string>(6).fill(orbRingColor));

  const haloProps = $derived(resolveTonePose(statusTone));
  const particleCount = 60;

  const ariaStatusText = $derived.by(() => {
    if (viewState.snapshot.state === 'recording') return 'Recording';
    if (viewState.snapshot.state === 'transcribing') return 'Transcribing';
    if (viewState.snapshot.state === 'done' && viewState.terminalStateVisible) return 'Transcript ready';
    return '';
  });
  const ariaAlertText = $derived(hasVisibleError ? (viewState.actionError ?? '') : '');

  let motionFrame: number | undefined;
  let motionGeneration = 0;
  let mounted = false;

  const controller = createOverlayController(viewState, {
    reducedMotion: () => reducedMotion, refreshPreviewWindow, clearTerminalStateTimer, showTerminalStateTemporarily
  });
  const { handleRuntimeEvent } = controller;

  onMount(() => {
    mounted = true;
    const preference = window.matchMedia('(prefers-reduced-motion: reduce)');
    const syncMotion = () => { reducedMotion = preference.matches; controller.motionChanged(); syncShell(); };
    const syncVisibility = () => { documentVisible = !document.hidden; };
    syncMotion(); syncVisibility();
    preference.addEventListener('change', syncMotion);
    document.addEventListener('visibilitychange', syncVisibility);
    runtime = createRuntime({ hotkeyOwner: true });
    const unsubscribe = runtime.subscribe(handleRuntimeEvent);

    void overlayWindow.initialize().then(() => { if (mounted) syncShell(); });

    runtime
      .getSnapshot()
      .then((nextSnapshot) => {
        if (mounted) viewState.snapshot = nextSnapshot;
      })
      .catch((error: unknown) => {
        if (mounted) viewState.actionError = error instanceof Error ? error.message : 'Runtime failed';
      });

    return () => {
      mounted = false;
      preference.removeEventListener('change', syncMotion);
      document.removeEventListener('visibilitychange', syncVisibility);
      motionGeneration++;
      if (motionFrame !== undefined) cancelAnimationFrame(motionFrame);
      unsubscribe();
      void runtime?.dispose();
      clearTerminalStateTimer();
      controller.dispose();
    };
  });

  $effect(() => {
    if (typeof document === 'undefined') return;
    document.documentElement.style.setProperty('--echo-window-radius', windowRadius);
  });

  $effect(() => {
    const speed = orbMotionSpeed;
    const color = orbRingColor;
    const suppressed = reducedMotion || !documentVisible || expanded;
    const generation = ++motionGeneration;
    if (motionFrame !== undefined) cancelAnimationFrame(motionFrame);
    motionFrame = undefined;
    if (suppressed) {
      motion.speed = speed;
      motion.colors.fill(color);
      return;
    }
    if (motion.speed === speed && motion.colors[0] === color) return;
    const fromSpeed = motion.speed;
    readColor(motion.colors[0], colorFrom);
    readColor(color, colorTarget);
    const start = performance.now();
    const tick = (now: number) => {
      if (!mounted || generation !== motionGeneration) return;
      const t = Math.min((now - start) / 160, 1);
      motion.speed = fromSpeed + (speed - fromSpeed) * (1 - Math.pow(1 - t, 3));
      colorSample(t);
      motionFrame = t < 1 ? requestAnimationFrame(tick) : undefined;
    };
    motionFrame = requestAnimationFrame(tick);
    return () => {
      motionGeneration++;
      if (motionFrame !== undefined) cancelAnimationFrame(motionFrame);
      motionFrame = undefined;
    };
  });

  function syncShell() {
    viewState.shell = reduceShell(viewState.shell, {
      type: 'sync', hasText: Boolean(viewState.preview.text || viewState.transcriptionNotice), ready: overlayWindow.previewReady, reducedMotion
    });
  }

  function refreshPreviewWindow() {
    syncShell();
    void overlayWindow.setPreview(retainedPreview).then(syncShell);
  }

  function finishShell(generation: number) {
    const next = reduceShell(viewState.shell, { type: 'finished', generation });
    if (next === viewState.shell) return;
    viewState.shell = next;
    refreshPreviewWindow();
  }

  function showTerminalStateTemporarily() {
    clearTerminalStateTimer();
    const generation = terminalGeneration;
    viewState.terminalStateVisible = true;
    terminalStateTimer = setTimeout(() => {
      if (!mounted || generation !== terminalGeneration) return;
      terminalStateTimer = undefined;
      viewState.terminalStateVisible = false;
      viewState.copyStatus = null;
      viewState.actionError = null;
      if (viewState.snapshot.state === 'done' || viewState.snapshot.state === 'error') {
        viewState.snapshot = { ...viewState.snapshot, state: 'idle', lastError: undefined };
      }
    }, TERMINAL_STATE_MS);
  }

  function clearTerminalStateTimer() {
    terminalGeneration++;
    clearTimeout(terminalStateTimer);
    terminalStateTimer = undefined;
  }

  async function copyLastTranscript(event?: MouseEvent) {
    event?.stopPropagation();
    const transcript = viewState.snapshot.lastTranscript;
    if (!runtime || !transcript?.id) return;

    try {
      await runtime.copyTranscript(transcript.id);
      viewState.copyStatus = 'Copied';
    } catch (error: unknown) {
      viewState.actionError = error instanceof Error ? error.message : errorLabels.clipboard_failed;
      viewState.terminalStateVisible = true;
      viewState.copyStatus = null;
    }
  }

  async function stop() {
    if (!runtime) return;
    viewState.actionError = null;
    viewState.snapshot = await runtime.stopRecording();
  }

</script>

<svelte:head>
  <title>Echo Overlay</title>
</svelte:head>

<OverlaySurface
  model={{
    actionError: viewState.actionError,
    activityText,
    animationSpeed: orbMotionSpeed,
    motion,
    documentVisible,
    ariaAlertText,
    ariaStatusText,
    copyStatus: viewState.copyStatus,
    currentModelText,
    errorLabel: viewState.snapshot.lastError ? errorLabels[viewState.snapshot.lastError] : 'Something went wrong',
    expanded,
    haloMode: haloProps.haloMode,
    haloPose: haloProps.pose,
    hasVisibleError,
    hasVisibleSuccess,
    isBusy,
    isLoading,
    particleCount,
    previewText: overlayWindow.previewReady ? viewState.preview.text : '',
    previewWords: viewState.preview.words,
    shell: viewState.shell,
    previewFading: viewState.preview.phase === 'fading',
    reducedMotion,
    ringColors: orbRingColors,
    snapshot: viewState.snapshot,
    statusTone,
    warningText: expanded || overlayWindow.previewReady ? presentation.warningText : '',
    transcriptionNotice: expanded || overlayWindow.previewReady ? viewState.transcriptionNotice : '',
    warningAnnouncement: viewState.warningAnnouncement
  }}
  onCollapse={(event) => void overlayWindow.collapseFrom(event).then(syncShell)}
  onCopy={copyLastTranscript}
  onMorphFinished={finishShell}
  onDragEnd={() => overlayWindow.handleDragEnd()}
  onDragMove={(event) => void overlayWindow.handleDragMove(event)}
  onDragStart={(event) => overlayWindow.handleDragStart(event)}
  onStop={stop}
  onToggle={(event) => void overlayWindow.toggle(event).then(syncShell)}
/>
