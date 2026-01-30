import { Time } from "./time";

export interface Cell<T, S> {
  get current(): T;
  at(time: Time): T;
  update(state: S, time: Time): void;
}

export type Serializable =
  | string
  | number
  | boolean
  | null
  | Serializable[]
  | { [key: string]: Serializable }
  | { toJSON(): Serializable };

export class ExtractorCell<T extends Serializable, S> implements Cell<T, S> {
  private cache = new Map<Time, T>();
  constructor(
    private runtime: Runtime<S>,
    private extract: (state: S) => T,
  ) {
    runtime.register_extractor(this);
  }

  update(state: S, time: Time): void {
    const value = this.extract(state);
    this.cache.set(time, value);
  }

  get current(): T {
    const value = this.cache.get(this.runtime.time);
    if (value === undefined) {
      throw new Error(
        `no cell value available in current state (this is a bug in the runtime)`,
      );
    } else {
      return value;
    }
  }

  at(time: Time): T {
    if (time.is_before(this.runtime.time)) {
      const value = this.cache.get(time);
      if (value === undefined) {
        throw new Error("cannot get value from unknown time");
      }
      return value;
    } else if (this.runtime.time.is_before(time)) {
      throw new Error("cannot get cell value from the future");
    } else {
      return this.current;
    }
  }

  as_js_function(): string {
    return this.extract.toString();
  }
}

export class TimeCell implements Cell<Time, any> {
  constructor(private runtime: Runtime<any>) {}

  update(): void {}

  get current(): Time {
    return this.runtime.time;
  }

  at(time: Time): Time {
    return time;
  }
}

export class Runtime<S> {
  private current_state: { state: S; time: Time } | null = null;
  private cells: ExtractorCell<any, S>[] = [];

  get time(): Time {
    if (this.current_state === null) {
      throw new Error("runtime has no current time");
    }
    return this.current_state.time;
  }

  register_state(state: S, timestamp_ms: number): Time {
    const time_new = new Time(timestamp_ms);
    if (
      this.current_state !== null &&
      time_new.is_before(this.current_state.time)
    ) {
      throw new Error("non-monotonic time update in register_state");
    }
    this.current_state = { state, time: time_new };
    for (const cell of this.cells) {
      cell.update(state, time_new);
    }
    return time_new;
  }

  register_extractor(cell: ExtractorCell<any, S>) {
    this.cells.push(cell);
  }

  reset() {
    this.current_state = null;
    this.cells = [];
  }
}
