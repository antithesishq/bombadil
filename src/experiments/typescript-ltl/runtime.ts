export interface State {
  document: HTMLDocument;
}

export type Time = number;

export class Runtime<S = State> {
  private index_next = 0;
  private current_state: { state: S; index: number } | null = null;

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
    return this.current_state.index;
  }

  register_state(state: S): Time {
    this.current_state = { state, index: this.index_next };
    this.index_next += 1;
    return this.index_next;
  }

  reset() {
    this.index_next = 0;
    this.current_state = null;
  }
}

export let runtime_default = new Runtime<State>();
