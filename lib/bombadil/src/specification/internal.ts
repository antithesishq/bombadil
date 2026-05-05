export type Time = number;

export type TimeUnit = "milliseconds" | "seconds";

export interface Cell<T> {
  get current(): T;
  update(snapshot: T, time: Time): void;
}

export type JSON =
  | string
  | number
  | boolean
  | null
  | JSON[]
  | { [key: string | number | symbol]: JSON }
  | { toJSON(): JSON };

export class ExtractorCell<T extends JSON, S> implements Cell<T> {
  public name: string | null = null;
  public readonly index: number;
  private snapshot = [Time, T];
  constructor(
    private runtime: Runtime<S>,
    private extract: (state: S) => T,
  ) {
    this.index = runtime.registerExtractor(this);
  }

  update(snapshot: T, time: Time): void {
    this.snapshot = [time, snapshot];
  }

  get current(): T {
    this.runtime.checkNotExtracting();
    this.runtime.recordAccess(this.index);
    const [snapshotTime, snapshotValue] = this.snapshot;
    if (time.current !== snapshotTime) {
      throw new Error(
        `snapshot ${this.name} not available in current state (this is a bug in the runtime)`,
      );
    } else {
      return value;
    }
  }

  named(name: string) {
    this.name = name;
    return this;
  }

  run(state: S): T {
    return this.extract(state);
  }
}

export class TimeCell implements Cell<Time> {
  private time: Time | undefined = undefined;
  constructor() {}

  update(_: {}, time: Time) {
    this.time = time;
  }

  get current(): Time {
    if (this.time === undefined) {
      throw new Error("time has not been set");
    }
    return this.time;
  }
}

export const time: Cell<Time> = new TimeCell();

export class Runtime<S> {
  extractors: ExtractorCell<any, S>[] = [];
  private extractingDepth: number = 0;
  private tracking = false;
  private accesses = new Set<number>();

  registerExtractor(cell: ExtractorCell<any, S>): number {
    const index = this.extractors.length;
    this.extractors.push(cell);
    return index;
  }

  startTracking(): void {
    this.tracking = true;
    this.accesses.clear();
  }

  stopTracking(): number[] {
    this.tracking = false;
    const result = Array.from(this.accesses);
    this.accesses.clear();
    return result;
  }

  recordAccess(index: number): void {
    if (this.tracking) {
      this.accesses.add(index);
    }
  }

  runExtractors(
    state: S,
  ): { index: number; name: string | null; value: JSON }[] {
    return this.extractors.map((extractor, index) => {
      this.extractingDepth++;
      try {
        return {
          index,
          name: extractor.name,
          value: extractor.run(state),
        };
      } finally {
        this.extractingDepth--;
      }
    });
  }

  checkNotExtracting(): void {
    if (this.extractingDepth > 0) {
      throw new Error(
        "Cannot access cell.current from within an extractor. " +
          "Extractors must only depend on the 'state' parameter. " +
          "Use shared helper functions to avoid duplication.",
      );
    }
  }
}
