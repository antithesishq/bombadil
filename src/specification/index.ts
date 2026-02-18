import {
  type JSON,
  ExtractorCell,
  Runtime,
  type TimeUnit,
  type Cell,
} from "@antithesishq/bombadil/internal";

/** @internal */
export const runtime_default = new Runtime<State>();

// Reexports
export { time, type Cell } from "@antithesishq/bombadil/internal";
export {
  actions,
  weighted,
  type Action,
  type Generator,
  type Point,
  ActionGenerator,
  from,
  strings,
  emails,
  integers,
  keycodes,
} from "@antithesishq/bombadil/actions";

import type { Action } from "@antithesishq/bombadil/actions";

export class Formula {
  not(): Formula {
    return new Not(this);
  }
  and(that: IntoFormula): Formula {
    return new And(this, now(that));
  }
  or(that: IntoFormula): Formula {
    return new Or(this, now(that));
  }
  implies(that: IntoFormula): Formula {
    return new Implies(this, now(that));
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
    public left: Formula,
    public right: Formula,
  ) {
    super();
  }

  override toString() {
    return `${this.left}.implies(${this.right})`;
  }
}

export class Not extends Formula {
  constructor(public subformula: Formula) {
    super();
  }
  override toString() {
    return `!(${this.subformula.toString()})`;
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
  constructor(
    public bound_millis: number | null,
    public subformula: Formula,
  ) {
    super();
  }

  within(n: number, unit: TimeUnit): Formula {
    if (this.bound_millis !== null) {
      throw new Error("time bound is already set for `always`");
    }
    let duration_millis: number;
    switch (unit) {
      case "milliseconds":
        duration_millis = n;
        break;
      case "seconds":
        duration_millis = n * 1000;
        break;
    }
    return new Always(duration_millis, this.subformula);
  }

  override toString() {
    return this.bound_millis === null
      ? `always(${this.subformula})`
      : `always(${this.subformula}).within(${this.bound_millis}, "milliseconds")`;
  }
}

export class Eventually extends Formula {
  constructor(
    public bound_millis: number | null,
    public subformula: Formula,
  ) {
    super();
  }

  within(n: number, unit: TimeUnit): Formula {
    if (this.bound_millis !== null) {
      throw new Error("time bound is already set for `eventually`");
    }
    let duration_millis: number;
    switch (unit) {
      case "milliseconds":
        duration_millis = n;
        break;
      case "seconds":
        duration_millis = n * 1000;
        break;
    }
    return new Eventually(duration_millis, this.subformula);
  }

  override toString() {
    return this.bound_millis === null
      ? `eventually(${this.subformula})`
      : `eventually(${this.subformula}).within(${this.bound_millis}, "milliseconds")`;
  }
}

export class Thunk extends Formula {
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

type IntoFormula = (() => Formula | boolean) | Formula;

export function not(value: IntoFormula) {
  return new Not(now(value));
}

export function now(x: IntoFormula): Formula {
  if (typeof x === "function") {
    const pretty = x
      .toString()
      .replace(/^\(\)\s*=>\s*/, "")
      .replaceAll(/(\|\||&&)/g, (_, operator) => "\n  " + operator);

    function lift_result(result: Formula | boolean): Formula {
      return typeof result === "boolean" ? new Pure(pretty, result) : result;
    }

    return new Thunk(pretty, () => lift_result(x()));
  }

  return x;
}

export function next(x: IntoFormula): Formula {
  return new Next(now(x));
}

export function always(x: IntoFormula): Always {
  return new Always(null, now(x));
}

export function eventually(x: IntoFormula): Eventually {
  return new Eventually(null, now(x));
}

export function extract<T extends JSON>(query: (state: State) => T): Cell<T> {
  return new ExtractorCell<T, State>(runtime_default, query);
}

export interface State {
  document: HTMLDocument;
  window: Window;
  navigation_history: {
    back: NavigationEntry[];
    current: NavigationEntry;
    forward: NavigationEntry[];
  };
  errors: {
    uncaught_exceptions: {
      text: string;
      line: number;
      column: number;
      url: string | null;
      stacktrace:
        | { name: string; line: number; column: number; url: string }[]
        | null;
    }[];
  };
  console: ConsoleEntry[];
  last_action: Action | null;
}

export type NavigationEntry = {
  id: number;
  title: string;
  url: string;
};

export type ConsoleEntry = {
  timestamp: number;
  level: "warning" | "error";
  args: JSON[];
};
