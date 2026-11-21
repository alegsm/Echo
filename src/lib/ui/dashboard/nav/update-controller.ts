import type { UpdateClient, UpdateEnvelope, UpdateStatus } from '#lib/runtime/desktop/update-client.ts';

/** What the sidebar button shows. `percent` is null while the download size is unknown. */
export type UpdateView =
  | { kind: 'hidden' }
  | { kind: 'available' | 'waiting' | 'installing' | 'failed'; version: string }
  | { kind: 'downloading'; version: string; percent: number | null };

type UpdateState = { view: UpdateView };

function viewOf(status: UpdateStatus): UpdateView {
  switch (status.state) {
    case 'none': return { kind: 'hidden' };
    case 'waiting_for_idle': return { kind: 'waiting', version: status.version };
    case 'downloading': {
      const { version, downloadedBytes, totalBytes } = status;
      const percent = totalBytes ? Math.min(100, Math.floor((downloadedBytes / totalBytes) * 100)) : null;
      return { kind: 'downloading', version, percent };
    }
    default: return { kind: status.state, version: status.version };
  }
}

/** The button's accessible name and title. */
export function updateLabel(view: Exclude<UpdateView, { kind: 'hidden' }>) {
  switch (view.kind) {
    case 'available': return `Update Echo to v${view.version}`;
    case 'downloading': return `Downloading Echo v${view.version}${view.percent === null ? '' : `, ${view.percent}%`}`;
    case 'waiting': return `Echo v${view.version} installs when this dictation ends`;
    case 'installing': return `Installing Echo v${view.version}`;
    case 'failed': return 'Update failed. Try again';
  }
}

/** Live-region text: the download start, the wait and the failure, never each percent. */
export function updateAnnouncement(view: UpdateView) {
  switch (view.kind) {
    case 'downloading': return `Downloading Echo v${view.version}`;
    case 'waiting':
    case 'failed': return updateLabel(view);
    default: return '';
  }
}

/** True while native work runs and activating the button does nothing. */
export function updateBusy(view: UpdateView) {
  return view.kind === 'downloading' || view.kind === 'waiting' || view.kind === 'installing';
}

/**
 * Projects native update status into `state.view`. It subscribes before the first read and applies
 * only envelopes newer than the last one applied, so a late read or event cannot roll the view back.
 */
export function createUpdateController(client: UpdateClient, state: UpdateState) {
  let sequence = -1;

  function apply(envelope: UpdateEnvelope) {
    if (envelope.sequence <= sequence) return;
    sequence = envelope.sequence;
    state.view = viewOf(envelope.status);
  }

  /** Requests the install from the available or failed state; native ignores any other request too. */
  async function install() {
    if (state.view.kind !== 'available' && state.view.kind !== 'failed') return;
    try {
      apply(await client.install());
    } catch {
      // Native reports download and install failures as a `failed` status. A rejected request
      // changed nothing natively, so the current offer stays and the user can activate it again.
    }
  }

  function mount() {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const stop = await client.listen(apply);
        if (disposed) return stop();
        unlisten = stop;
        apply(await client.status());
      } catch {
        // Updates are optional: an unreadable status keeps the button hidden and no error reaches the user.
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }

  return { install, mount };
}
