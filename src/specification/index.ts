import {
  type JSON,
  ExtractorCell,
  type Cell,
  Runtime,
  Duration,
  type TimeUnit,
} from "bombadil/internal";

/** @internal */
export const runtime_default = new Runtime<State>();

// Reexports
export { time, type Cell } from "bombadil/internal";

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
  constructor(
    private pretty: string,
    public value: boolean,
  ) {
    super();
  }

  override toString() {
    return this.pretty;
  }
}

export class And extends Formula {
  constructor(
    public left: Formula,
    public right: Formula,
  ) {
    super();
  }

  override toString() {
    return `(${this.left}) && (${this.right})`;
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

  override toString() {
    return `${this.antecedent}.implies(${this.consequent}`;
  }
}

export class Not extends Formula {
  constructor(public subformula: Formula) {
    super();
  }
}

export class Next extends Formula {
  constructor(public subformula: Formula) {
    super();
  }

  override toString() {
    return `next(${this.subformula})`;
  }
}

export class Always extends Formula {
  constructor(public subformula: Formula) {
    super();
  }

  override toString() {
    return `always(${this.subformula})`;
  }
}

export class Eventually extends Formula {
  constructor(
    public timeout: Duration,
    public subformula: Formula,
  ) {
    super();
  }

  override toString() {
    return `eventually(${this.subformula}).within(${this.timeout.milliseconds}, "milliseconds")`;
  }
}

export class Contextful extends Formula {
  constructor(
    private pretty: string,
    public apply: () => Formula,
  ) {
    super();
  }

  override toString() {
    return this.pretty;
  }
}

type IntoCondition = (() => Formula | boolean) | Formula;

export function not(value: IntoCondition) {
  return new Not(condition(value));
}

export function condition(x: IntoCondition): Formula {
  if (typeof x === "function") {
    const pretty = x
      .toString()
      .replace(/^\(\)\s*=>\s*/, "")
      .replaceAll(/(\|\||&&)/g, (_, operator) => "\n  " + operator);

    function lift_result(result: Formula | boolean): Formula {
      return typeof result === "boolean" ? new Pure(pretty, result) : result;
    }

    return new Contextful(pretty, () => lift_result(x()));
  }

  return x;
}

export function next(x: IntoCondition): Formula {
  return new Next(condition(x));
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

export function extract<T extends JSON>(query: (state: State) => T): Cell<T> {
  return new ExtractorCell<T, State>(runtime_default, query);
}

export interface State {
  document: HTMLDocument;
  window: Window;
  errors: {
    uncaught_exception: JSON;
    unhandled_promise_rejection: JSON;
  };
  console: ConsoleEntry[];
}

export type ConsoleEntry = {
  timestamp: number;
  level: "warning" | "error";
  args: JSON[];
};
