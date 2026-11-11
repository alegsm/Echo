export type DiagnosticLevel = 'info' | 'warn' | 'error';
export type DiagnosticScalar = string | number | boolean | null;

export type DiagnosticEvent = {
  area: string;
  event: string;
  attemptId?: string;
  level?: DiagnosticLevel;
  fields?: Record<string, DiagnosticScalar | undefined>;
};

export type DiagnosticStatus = {
  state: 'off' | 'ready' | 'error';
  message: string;
};

type DiagnosticWriterOptions = {
  isEnabled: () => boolean;
  sink: (request: DiagnosticEvent) => Promise<boolean>;
  onStatus: (status: DiagnosticStatus) => void;
};

/** Serializes structured requests. Native code owns filtering and provenance. */
export function createDiagnosticEventWriter(options: DiagnosticWriterOptions) {
  return new DiagnosticEventWriter(options);
}

class DiagnosticEventWriter {
  private writeQueue = Promise.resolve<boolean>(false);

  constructor(private readonly options: DiagnosticWriterOptions) {}

  async write(event: DiagnosticEvent): Promise<boolean> {
    if (!this.loggingIsEnabled()) return false;
    this.writeQueue = this.writeQueue.then(() => this.writeNext(event));
    return this.writeQueue;
  }

  private async writeNext(event: DiagnosticEvent): Promise<boolean> {
    if (!this.loggingIsEnabled()) return false;
    try {
      const accepted = await this.options.sink(event);
      if (!this.loggingIsEnabled()) return false;
      if (!accepted) {
        this.options.onStatus({ state: 'off', message: 'Detailed logging is off.' });
        return false;
      }
      this.options.onStatus({ state: 'ready', message: 'Native logger accepted a detailed event.' });
      return true;
    } catch {
      if (!this.loggingIsEnabled()) return false;
      this.options.onStatus({ state: 'error', message: 'Could not write diagnostic log.' });
      return false;
    }
  }

  private loggingIsEnabled(): boolean {
    if (this.options.isEnabled()) return true;
    this.options.onStatus({ state: 'off', message: 'Detailed logging is off.' });
    return false;
  }
}
