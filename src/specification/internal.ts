export type Time = number;

export type TimeUnit = "milliseconds" | "seconds";

export interface Cell<T> {
  get current(): T;
  at(time: Time): T;
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
  private snapshots = new Map<Time, T>();
  constructor(
    runtime: Runtime<S>,
    private extract: (state: S) => T,
  ) {
    runtime.register_extractor(this);
  }

  update(snapshot: T, time: Time): void {
    this.snapshots.set(time, snapshot);
  }

  get current(): T {
    const value = this.snapshots.get(time.current);
    if (value === undefined) {
      throw new Error(
        `no cell value available in current state (this is a bug in the runtime)`,
      );
    } else {
      return value;
    }
  }

  at(other: Time): T {
    if (other < time.current) {
      const value = this.snapshots.get(other);
      if (value === undefined) {
        throw new Error("cannot get value from unknown time");
      }
      return value;
    } else if (time.current < other) {
      throw new Error("cannot get cell value from the future");
    } else {
      return this.current;
    }
  }

  as_js_function(): string {
    return this.extract.toString();
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

  at(time: Time): Time {
    return time;
  }
}

export const time: Cell<Time> = new TimeCell();

export class Runtime<S> {
  extractors: ExtractorCell<any, S>[] = [];

  register_extractor(cell: ExtractorCell<any, S>) {
    this.extractors.push(cell);
  }
}
