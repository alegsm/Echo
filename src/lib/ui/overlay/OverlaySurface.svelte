<script lang="ts">
  import { untrack } from 'svelte';
  import OutlineParticles from './OutlineParticles.svelte';
  import { MORPH_MS, type ShellPhase } from './overlay-shell-phase';
  import ParticleHalo from './ParticleHalo.svelte';
  import type { EchoSnapshot } from '#lib/core/types.ts';
  import type { ToneHaloMode, TonePose } from './state-tone';
  import { PREVIEW_CARET_FADE_MS, previewCaretState, previewContentOffset, previewWordAnimation, type PreviewWord } from './overlay-preview';

  type SurfaceModel = {
    actionError: string | null;
    activityText: string;
    animationSpeed: number;
    documentVisible?: boolean;
    motion?: { colors: string[]; speed: number };
    ariaAlertText: string;
    ariaStatusText: string;
    copyStatus: string | null;
    currentModelText: string;
    errorLabel: string;
    expanded: boolean;
    haloMode?: ToneHaloMode;
    haloPose: TonePose;
    hasVisibleError: boolean;
    hasVisibleSuccess: boolean;
    isBusy: boolean;
    isLoading: boolean;
    particleCount: number;
    shell: ShellPhase;
    previewText?: string;
    previewWords: PreviewWord[];
    previewFading?: boolean;
    reducedMotion?: boolean;
    ringColors: string[];
    snapshot: EchoSnapshot;
    statusTone: string;
    warningText?: string;
    transcriptionNotice?: string;
    warningAnnouncement?: string;
  };

  type Props = {
    model: SurfaceModel;
    onCollapse(event?: MouseEvent): void;
    onCopy(event?: MouseEvent): void;
    onDragEnd(): void;
    onDragMove(event: PointerEvent): void;
    onDragStart(event: PointerEvent): void;
    onMorphFinished(generation: number): void;
    onStop(): void;
    onToggle(event: MouseEvent): void;
  };

  let { model, onCollapse, onCopy, onDragEnd, onDragMove, onDragStart, onMorphFinished, onStop, onToggle }: Props = $props();

  const shell = $derived(model.shell);
  const expanded = $derived(model.expanded);
  const documentVisible = $derived(model.documentVisible !== false);
  const reducedMotion = $derived(Boolean(model.reducedMotion));

  let haloEligible = $state(true);
  let outlineEligible = $state(false);

  function trackLights() {
    const phase = shell.phase;
    const generation = shell.generation;
    const suppressed = expanded || !documentVisible || reducedMotion;
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    haloEligible = !suppressed && phase !== 'landed';
    outlineEligible = !suppressed && phase !== 'resting';
    const delay = phase === 'outward' ? 240 : phase === 'returning' ? 80 : 0;
    if (!suppressed && delay) timer = setTimeout(() => {
      if (!active || model.shell.generation !== generation) return;
      if (phase === 'outward') haloEligible = false;
      else outlineEligible = false;
    }, delay);
    return () => { active = false; clearTimeout(timer); };
  }

  function trackShell(node: HTMLElement) {
    const { phase, generation } = shell;
    const hidden = expanded || !documentVisible || reducedMotion;
    if (phase !== 'outward' && phase !== 'returning') return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let finished = false;
    const finish = () => {
      if (finished || model.shell.generation !== generation) return;
      finished = true;
      onMorphFinished(generation);
    };
    const landed = (event: TransitionEvent) => {
      if (event.target === node && event.propertyName === 'top') finish();
    };
    node.addEventListener('transitionend', landed);
    // Hidden details have no CSS transition. Also cover a reversal before its first paint.
    if (hidden) queueMicrotask(finish);
    else timer = setTimeout(finish, MORPH_MS + 32);
    return () => {
      finished = true;
      node.removeEventListener('transitionend', landed);
      clearTimeout(timer);
    };
  }

  const wordAnimations = new WeakMap<HTMLElement, ReturnType<typeof previewWordAnimation>>();

  function revealWord(node: HTMLElement, word: PreviewWord) {
    if (!documentVisible) {
      wordAnimations.delete(node);
      node.classList.add('preview-word-suspended');
      node.style.animation = 'none';
      return;
    }
    node.classList.remove('preview-word-suspended');
    const previous = wordAnimations.get(node);
    const animation = previewWordAnimation(word, performance.now(), reducedMotion, previous);
    if (animation === previous) return;
    wordAnimations.set(node, animation);
    const delay = animation.delayMs;
    node.style.setProperty('--reveal-duration', `${word.revealDurationMs}ms`);
    node.style.setProperty('--reveal-delay', `${delay ?? 0}ms`);
    node.classList.toggle('preview-word-settled', delay === null);
    if (delay === null) node.style.animation = 'none';
    else {
      node.style.removeProperty('animation');
      node.style.animationDuration = `${word.revealDurationMs}ms`;
      node.style.animationDelay = `${delay}ms`;
    }
  }

  function measurePreview(clip: HTMLElement) {
    const content = clip.firstElementChild as HTMLElement;
    const reduce = reducedMotion;
    const suppressed = expanded || !documentVisible;
    let frame: number | undefined;
    let active = true;
    let dirty = true;
    let fullMeasure = true;
    let clipHeight = 0;
    let lineHeight = 0;
    let words = untrack(() => model.previewWords);
    let caret: HTMLElement | null = null;
    let caretEnd = 0;
    const caretOutput = { wordIndex: 0, progress: 0, opacity: 0 };
    const rows: { left: number; top: number; width: number; bottom: number; revealAt: number }[] = [];

    function measure() {
      words = untrack(() => model.previewWords);
      clipHeight = clip.clientHeight;
      lineHeight = parseFloat(getComputedStyle(content).lineHeight);
      const contentRect = content.getBoundingClientRect();
      const nodes = content.querySelectorAll<HTMLElement>('[data-preview-word]');
      // Appends can re-centre or wrap the last row; earlier rows retain their layout.
      let first = 0;
      if (!fullMeasure && rows.length && nodes.length >= rows.length) {
        first = rows.length - 1;
        while (first > 0 && rows[first - 1].top === rows[first].top) first--;
      }
      rows.length = Math.min(rows.length, nodes.length);
      for (let i = first; i < nodes.length; i++) {
        const node = nodes[i];
        const rect = node.getBoundingClientRect();
        const row = rows[i] ?? (rows[i] = { left: 0, top: 0, width: 0, bottom: 0, revealAt: 0 });
        row.left = rect.left - contentRect.left;
        row.top = rect.top - contentRect.top;
        row.width = rect.width;
        row.bottom = node.offsetTop + node.offsetHeight;
        row.revealAt = Number(node.dataset.revealAtMs);
      }
      caret = content.querySelector<HTMLElement>('[data-preview-caret]');
      const last = words.at(-1);
      caretEnd = last ? last.arrivedAt + last.revealDelayMs + last.revealDurationMs + PREVIEW_CARET_FADE_MS : 0;
      dirty = false;
      fullMeasure = false;
    }

    function advance(now: number) {
      frame = undefined;
      if (!active || suppressed) return;
      if (dirty) measure();
      // All layout reads above precede every DOM write in this pass.
      const offset = previewContentOffset(rows, clipHeight, now, reduce);
      content.style.setProperty('--preview-offset', `${offset}px`);
      clip.classList.toggle('overflowing', offset < -lineHeight / 2);
      if (caret) {
        const state = previewCaretState(words, now, reduce, caretOutput);
        caret.style.opacity = `${state?.opacity ?? 0}`;
        if (state) {
          const row = rows[state.wordIndex];
          caret.style.transform = `translate(${row.left + row.width * state.progress}px, ${row.top}px)`;
        }
      }
      if (!reduce && now < caretEnd) frame = requestAnimationFrame(advance);
    }

    function invalidate(all = false) {
      if (!active || suppressed) return;
      dirty = true;
      fullMeasure ||= all;
      if (frame === undefined) frame = requestAnimationFrame(advance);
    }
    const observer = new ResizeObserver(() => invalidate(true));
    observer.observe(clip);
    // Content height can change on append; the mutation observer invalidates its last row.
    const wordsObserver = new MutationObserver(() => invalidate());
    wordsObserver.observe(content, { childList: true, subtree: true });
    const fonts = document.fonts;
    const fontChanged = () => invalidate(true);
    fonts?.addEventListener('loadingdone', fontChanged);
    if (!suppressed) {
      if (reduce) advance(performance.now());
      else invalidate(true);
    }
    return () => {
      active = false;
      observer.disconnect();
      wordsObserver.disconnect();
      fonts?.removeEventListener('loadingdone', fontChanged);
      if (frame !== undefined) cancelAnimationFrame(frame);
    };
  }
</script>

<main
  {@attach trackLights}
  data-state={model.snapshot.state}
  data-visual-paused={model.expanded || model.documentVisible === false}
  data-reduced-motion={Boolean(model.reducedMotion)}
  data-tone={model.statusTone}
  data-mode="orb"
  data-morph-phase={model.shell.phase}
  class:expanded={model.expanded}
  class:has-preview={(Boolean(model.warningText) || Boolean(model.transcriptionNotice) || Boolean(model.previewText) || model.shell.phase !== 'resting') && !model.expanded}
>
  <section class="overlay" aria-label="Echo overlay">
    <div role="status" aria-live="polite" aria-atomic="true" class="sr-only">{model.ariaStatusText}</div>
    <div role="alert" aria-live="assertive" aria-atomic="true" class="sr-only">{model.ariaAlertText}</div>
    <div data-warning-announcement role="status" aria-live="polite" aria-atomic="true" class="sr-only">{model.warningAnnouncement ?? ''}</div>
    {#if model.warningText}
      <p data-recording-limit class="recording-limit">{model.warningText}</p>
    {/if}
    <div role="status" aria-live="polite" aria-atomic="true" class="sr-only">{model.transcriptionNotice ?? ''}</div>
    <div class="morph-anchor" aria-hidden="true">
      <div data-morph-shell data-morph-phase={model.shell.phase} class="orb-shell" aria-hidden="true" {@attach trackShell}>
        <OutlineParticles warning={model.statusTone === 'warning'} eligible={outlineEligible} reducedMotion={Boolean(model.reducedMotion)} />
        {#if (model.previewText || model.transcriptionNotice) && !model.expanded && model.shell.phase === 'landed'}
          <div data-preview-pill class="preview-pill" class:fading={model.previewFading} aria-hidden="true">
            {#if model.transcriptionNotice}
              <p data-transcription-notice class="transcription-notice">{model.transcriptionNotice}</p>
            {:else}
            <div data-preview-clip class="preview-clip" {@attach measurePreview}>
              <div data-preview-content class="preview-content">
                {#each model.previewWords as word (word.id)}
                  <span
                    data-preview-word
                    data-text={word.text}
                    data-word-id={word.id}
                    data-chunk-ordinal={word.chunkOrdinal}
                    data-reveal-delay-ms={word.revealDelayMs}
                    data-reveal-at-ms={word.arrivedAt + word.revealDelayMs}
                    data-reveal-duration-ms={word.revealDurationMs}
                    class="preview-word"
                    {@attach (node) => revealWord(node, word)}
                  >{word.text}</span>{' '}
                {/each}
                {#if !model.reducedMotion && !model.previewFading}
                  <span data-preview-caret class="preview-caret"></span>
                {/if}
              </div>
            </div>
            {/if}
          </div>
        {/if}
      </div>
    </div>
    <button
      class="matrix-shell"
      type="button"
      aria-label="Expand Echo overlay"
      aria-hidden={model.expanded}
      onpointerdown={onDragStart}
      onpointermove={(event) => void onDragMove(event)}
      onpointerup={onDragEnd}
      onpointercancel={onDragEnd}
      onclick={onToggle}
      ondblclick={(event) => event.stopPropagation()}
    >
      <span data-halo-ring class="orb-halo" aria-hidden="true">
        <ParticleHalo
          warning={model.statusTone === 'warning'}
          class="echo-threaded-ring"
          eligible={haloEligible}
          reducedMotion={Boolean(model.reducedMotion)}
          motion={model.motion}
          colors={model.ringColors}
          pose={model.haloPose}
          haloMode={model.haloMode}
          particleCount={model.particleCount}
          animationSpeed={model.animationSpeed}
          transitionMs={model.reducedMotion ? 0 : 300}
        />
      </span>
    </button>

    <div class="detail" aria-hidden={!model.expanded}>
      <div class="detail-content">
        {#if model.transcriptionNotice}<p class="transcription-notice">{model.transcriptionNotice}</p>{/if}
        {#if model.hasVisibleError}
          <p>Error</p>
          <span>{model.actionError || model.errorLabel}</span>
        {:else if model.hasVisibleSuccess}
          <p>Transcript ready</p>
          <span>{model.snapshot.lastTranscript?.text || 'Last transcript copied.'}</span>
        {:else if model.isBusy || model.isLoading}
          <p>{model.activityText}</p>
          <span>{model.currentModelText}</span>
        {:else}
          <p>Ready</p>
          <dl class="idle-grid" aria-label="Current overlay configuration">
            <div><dt>Model</dt><dd>{model.currentModelText}</dd></div>
            <div><dt>Shortcut</dt><dd>{model.snapshot.activeShortcut}</dd></div>
          </dl>
        {/if}
      </div>
      <div class="detail-actions">
        {#if model.hasVisibleSuccess && model.snapshot.lastTranscript?.id}
          <button type="button" onclick={onCopy}>{model.copyStatus || 'Copy'}</button>
        {:else if model.snapshot.state === 'recording'}
          <button type="button" onclick={onStop}>Stop</button>
        {/if}
      </div>
    </div>
    <button
      class="collapse-button"
      type="button"
      aria-label="Collapse Echo overlay"
      aria-hidden={!model.expanded}
      onclick={onCollapse}
    ></button>
  </section>
</main>
