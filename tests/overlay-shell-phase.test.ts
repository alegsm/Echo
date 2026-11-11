import { expect, test } from 'bun:test';
import { emptyShell, MORPH_MS, reduceShell, retainsPreviewWindow } from '../src/lib/ui/overlay/overlay-shell-phase';
import { emptyPreview, previewWordDelay, reduceOverlayPreview } from '../src/lib/ui/overlay/overlay-preview';

const arrive = { type: 'sync', hasText: true, ready: true } as const;
const clear = { type: 'sync', hasText: false, ready: true } as const;

test('placement gates arrival; appends preserve the outward leg; landing permits text', () => {
  expect(MORPH_MS).toBe(320);
  expect(reduceShell(emptyShell, { ...arrive, ready: false })).toBe(emptyShell);
  const outward = reduceShell(emptyShell, arrive);
  expect(outward.phase).toBe('outward');
  expect(reduceShell(outward, arrive)).toBe(outward);
  const landed = reduceShell(outward, { type: 'finished', generation: outward.generation });
  expect(landed.phase).toBe('landed');
  expect(reduceShell(landed, arrive)).toBe(landed);
});

test('fade retains the landed shell; clearing returns and only settling releases native geometry', () => {
  const outward = reduceShell(emptyShell, arrive);
  const landed = reduceShell(outward, { type: 'finished', generation: outward.generation });
  expect(reduceShell(landed, arrive)).toBe(landed);
  const returning = reduceShell(landed, clear);
  expect(returning.phase).toBe('returning');
  expect(retainsPreviewWindow(returning, false)).toBe(true);
  const resting = reduceShell(returning, { type: 'finished', generation: returning.generation });
  expect(resting.phase).toBe('resting');
  expect(retainsPreviewWindow(resting, false)).toBe(false);
  expect(retainsPreviewWindow(emptyShell, true)).toBe(true);
});

test('cancel during outward reverses; a new arrival reverses return and rejects stale completions', () => {
  const outward = reduceShell(emptyShell, arrive);
  const returning = reduceShell(outward, clear);
  expect(returning.phase).toBe('returning');
  expect(retainsPreviewWindow(returning, false)).toBe(true);
  expect(reduceShell(returning, { type: 'finished', generation: outward.generation })).toBe(returning);
  const reversed = reduceShell(returning, arrive);
  expect(reversed.phase).toBe('outward');
  expect(reduceShell(reversed, { type: 'finished', generation: returning.generation })).toBe(reversed);
  expect(retainsPreviewWindow(reversed, true)).toBe(true);
  expect(reduceShell(reversed, { type: 'finished', generation: reversed.generation }).phase).toBe('landed');
  const cancelled = reduceShell(reversed, clear);
  const resting = reduceShell(cancelled, { type: 'finished', generation: cancelled.generation });
  expect(retainsPreviewWindow(resting, false)).toBe(false);
});

test('reduced motion starts, reverses and ends synchronously', () => {
  const landed = reduceShell(emptyShell, { ...arrive, reducedMotion: true });
  expect(landed.phase).toBe('landed');
  expect(reduceShell(landed, { ...clear, reducedMotion: true }).phase).toBe('resting');
  const outward = reduceShell(emptyShell, arrive);
  expect(reduceShell(outward, { ...arrive, reducedMotion: true }).phase).toBe('landed');
});

test('words keep their arrival schedule while the shell travels and settle before mounting', () => {
  let preview = reduceOverlayPreview(emptyPreview, {
    type: 'partial_transcript_changed', projection: {
      dictationId: 'a', ordinal: 0, text: 'short', eligible: true, phase: 'active'
    }
  }, 1000);
  const first = preview.words[0];
  const outward = reduceShell(emptyShell, arrive);
  preview = reduceOverlayPreview(preview, {
    type: 'partial_transcript_changed', projection: {
      dictationId: 'a', ordinal: 1, text: 'short more words', eligible: true, phase: 'active'
    }
  }, 1080);
  expect(reduceShell(outward, arrive)).toBe(outward);
  expect(preview.words[0]).toBe(first);
  expect(previewWordDelay(first, 1320)).toBeNull();
  expect(preview.words[1].arrivedAt).toBe(1080);
});

test.each(['outward', 'landed'] as const)('release from %s returns once and rejects the old landing', (phase) => {
  const current = { phase, generation: 4 };
  const returning = reduceShell(current, { type: 'released' });
  expect(returning).toEqual({ phase: 'returning', generation: 5 });
  expect(reduceShell(returning, { type: 'released' })).toBe(returning);
  expect(reduceShell(returning, { type: 'finished', generation: 4 })).toBe(returning);
  expect(reduceShell(returning, clear)).toBe(returning);
  expect(reduceShell(returning, { type: 'finished', generation: 5 }).phase).toBe('resting');
  expect(reduceShell(current, { type: 'released', reducedMotion: true }).phase).toBe('resting');
});

test('release at rest is ignored; new recording text can reverse a loading return', () => {
  expect(reduceShell(emptyShell, { type: 'released' })).toBe(emptyShell);
  const returning = reduceShell(reduceShell(emptyShell, arrive), { type: 'released' });
  expect(reduceShell(returning, arrive).phase).toBe('outward');
});
