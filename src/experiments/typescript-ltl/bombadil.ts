import { runtime_default, Runtime, type State, type Time } from "./runtime";

export class Formula {
  and(that: IntoCondition): Formula {
    return new And(this, condition(that));
  }
  or(that: IntoCondition): Formula {
    return new Or(this, condition(that));
  }
  implies(that: IntoCondition): Formula {
    return new Implies(this, condition(that));
  }
}

export class Pure extends Formula {
  constructor(public value: boolean) {
    super();
  }
}

export class And extends Formula {
  constructor(
    public left: Formula,
    public right: Formula,
  ) {
    super();
  }
}

export class Or extends Formula {
  constructor(
    public left: Formula,
    public right: Formula,
  ) {
    super();
  }
}

export class Implies extends Formula {
  constructor(
    public antecedent: Formula,
    public consequent: Formula,
  ) {
    super();
  }
}

export class Not extends Formula {
  constructor(public subformula: Formula) {
    super();
  }
}

export class Always extends Formula {
  constructor(public subformula: Formula) {
    super();
  }
}

export class Eventually extends Formula {
  constructor(
    public seconds: Duration,
    public subformula: Formula,
  ) {
    super();
  }
}

export class Contextful extends Formula {
  constructor(public apply: () => Formula) {
    super();
  }
}

type IntoCondition = boolean | (() => Formula | boolean) | Formula;

export function not(value: IntoCondition) {
  return new Not(condition(value));
}

export function condition(x: IntoCondition): Formula {
  if (typeof x === "boolean") {
    return new Pure(x);
  }
  if (typeof x === "function") {
    return new Contextful(() => condition(x()));
  }

  return x;
}

export function always(x: IntoCondition): Formula {
  return new Always(condition(x));
}

export function eventually(x: IntoCondition) {
  return {
    within(n: number, unit: TimeUnit): Formula {
      return new Eventually(new Duration(n, unit), condition(x));
    },
  };
}

type Serializable =
  | string
  | number
  | boolean
  | null
  | Serializable[]
  | { [key: string]: Serializable }
  | { toJSON(): Serializable };

export interface Cell<T> {
  get current(): T;
  at(time: Time): T;
}

export class ExtractorCell<T extends Serializable, S = State>
  implements Cell<T>
{
  private cache = new Map<Time, T>();
  constructor(
    private runtime: Runtime<S>,
    private query: (state: S) => T,
  ) {}

  get current(): T {
    const value = this.cache.get(this.runtime.time);
    if (value === undefined) {
      const value = this.query(this.runtime.current);
      this.cache.set(this.runtime.time, value);
      return value;
    } else {
      return value;
    }
  }

  at(time: Time): T {
    if (time < this.runtime.time) {
      const value = this.cache.get(time);
      if (value === undefined) {
        throw new Error("cannot get value from unknown time");
      }
      return value;
    } else if (time > this.runtime.time) {
      throw new Error("cannot get cell value from the future");
    } else {
      return this.current;
    }
  }
}

export class TimeCell implements Cell<Time> {
  constructor(private runtime: Runtime<any>) {}

  get current(): Time {
    return this.runtime.time;
  }

  at(time: Time): Time {
    return time;
  }
}

export function extract<T extends Serializable>(
  query: (state: State) => T,
): ExtractorCell<T, State> {
  return new ExtractorCell(runtime_default, query);
}

export const time = new TimeCell(runtime_default);

export type TimeUnit = "milliseconds" | "seconds";

export class Duration {
  constructor(
    private n: number,
    unit: TimeUnit,
  ) {}
}
