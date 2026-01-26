import { Time } from "./time";

export interface State {
  document: HTMLDocument;
}

export class Runtime<S = State> {
  private index_next = 0;
  private current_state: { state: S; time: Time } | null = null;

  get current(): S {
    if (this.current_state === null) {
      throw new Error("runtime has no current state");
    }
    return this.current_state.state;
  }

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
    this.index_next += 1;
    return time_new;
  }

  reset() {
    this.index_next = 0;
    this.current_state = null;
  }
}

export let runtime_default = new Runtime<State>();
