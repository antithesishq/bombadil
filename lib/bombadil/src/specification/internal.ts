export type TimeUnit = "milliseconds" | "seconds";

/**
 * How a cell takes part in the state key. `excluded`: never in the key; its
 * snapshot is still recorded. `default`: in the key, and the tooling may advise
 * about it (the shipped extractors). `included`: in the key on the user's word
 * (user `extract` cells).
 */
export type StateKeyBehavior = "excluded" | "default" | "included";

export interface Cell<T> {
  get current(): T;
  update(snapshot: T): void;
  setStateKeyBehavior(behavior: StateKeyBehavior): this;
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
  /** Brand for the bundler's naming helper (`__bombadilNamed`). */
  public readonly __bombadilExtractor = true;
  public name: string | null = null;
  public stateKeyBehavior: StateKeyBehavior = "included";
  public readonly index: number;
  private snapshot: T | undefined;

  constructor(
    private runtime: Runtime<S>,
    private extract: (state: S) => T,
  ) {
    this.index = runtime.registerExtractor(this);
  }

  update(snapshot: T): void {
    this.snapshot = snapshot;
  }

  get current(): T {
    this.runtime.checkNotExtracting();
    this.runtime.recordAccess(this.index);
    if (this.snapshot === undefined) {
      throw new Error(
        `snapshot ${this.name} is not set for current state (this is a bug in the runtime)`,
      );
    } else {
      return this.snapshot;
    }
  }

  named(name: string) {
    this.name = name;
    return this;
  }

  setStateKeyBehavior(behavior: StateKeyBehavior): this {
    this.stateKeyBehavior = behavior;
    return this;
  }

  /**
   * A new extractor cell whose value is `f` of this one's, with its own name
   * and state-key behavior.
   */
  map<U extends JSON>(f: (value: T) => U): ExtractorCell<U, S> {
    return new ExtractorCell<U, S>(this.runtime, (state) => f(this.extract(state)));
  }

  /**
   * Runs the extractor and updates its cached value.
   */
  run(state: S): T {
    const value = this.extract(state);
    this.update(value);
    return value;
  }
}

export class RegisteredCustomAction<Args extends JSON[]> {
  constructor(
    public name: string,
    public run: (...args: Args) => Promise<void>,
  ) {}
}

type RunExtractorResult = {
  index: number;
  name: string | null;
  value: JSON;
  excluded: boolean;
};

export class Runtime<S> {
  extractors: ExtractorCell<any, S>[] = [];
  private extractingDepth: number = 0;
  private tracking = false;
  private accesses = new Set<number>();
  private customActions: Record<string, RegisteredCustomAction<any>> = {};

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

  runExtractors(state: S): RunExtractorResult[] {
    return this.extractors.map((extractor, index) => {
      this.extractingDepth++;
      try {
        return {
          index,
          name: extractor.name,
          value: extractor.run(state),
          excluded: extractor.stateKeyBehavior === "excluded",
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

  registerCustomAction<Args extends JSON[]>(
    action: RegisteredCustomAction<Args>,
  ) {
    if (action.name in this.customActions) {
      throw new Error(`Custom action "${action.name}" is already registered.`);
    }
    this.customActions[action.name] = action;
  }

  async runCustomAction(name: string, args: unknown[]): Promise<void> {
    const action = this.customActions[name];
    if (!action) {
      return Promise.reject(
        new Error(`Custom action "${name}" is not registered.`),
      );
    }
    return action.run(...args);
  }
}
