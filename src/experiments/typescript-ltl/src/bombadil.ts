import {
  Runtime,
  type Serializable,
  ExtractorCell,
  type Cell,
  TimeCell,
} from "./runtime";
import { Duration, type Time, type TimeUnit } from "./time";

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
    private rendered: string,
    public value: boolean,
  ) {
    super();
  }

  override toString() {
    return this.rendered;
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
    private string: string,
    public apply: () => Formula,
  ) {
    super();
  }

  override toString() {
    return this.string;
  }
}

type IntoCondition = (() => Formula | boolean) | Formula;

export function not(value: IntoCondition) {
  return new Not(condition(value));
}

export function condition(x: IntoCondition): Formula {
  if (typeof x === "function") {
    const rendered = x
      .toString()
      .replace(/^\(\)\s*=>\s*/, "")
      .replaceAll(/(\|\||&&)/g, (_, operator) => "\n  " + operator);

    function lift_result(result: Formula | boolean): Formula {
      return typeof result === "boolean" ? new Pure(rendered, result) : result;
    }

    return new Contextful(rendered, () => lift_result(x()));
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

export const runtime_default = new Runtime<State>();

export function extract<T extends Serializable>(
  query: (state: State) => T,
): Cell<T, State> {
  return new ExtractorCell(runtime_default, query);
}

export const time: Cell<Time, any> = new TimeCell(runtime_default);

export interface State {
  document: HTMLDocument;
}
