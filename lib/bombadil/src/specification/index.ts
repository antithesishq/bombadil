import {
  type JSON,
  ExtractorCell,
  Runtime,
  type TimeUnit,
  type Cell,
} from "@antithesishq/bombadil/internal";

/** @internal */
export const runtime = new Runtime<State>();

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

function durationMillis(n: number, unit: TimeUnit): number {
  switch (unit) {
    case "milliseconds":
      return n;
    case "seconds":
      return n * 1000;
  }
}

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
  until(that: IntoFormula): Until {
    return new Until(null, this, now(that));
  }
  release(that: IntoFormula): Release {
    return new Release(null, this, now(that));
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

  override toString() {
    return `(${this.left}) || (${this.right})`;
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
    return `!(${this.subformula})`;
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
    public boundMillis: number | null,
    public subformula: Formula,
  ) {
    super();
  }

  within(n: number, unit: TimeUnit): Formula {
    if (this.boundMillis !== null) {
      throw new Error("time bound is already set for `always`");
    }
    return new Always(durationMillis(n, unit), this.subformula);
  }

  override toString() {
    return this.boundMillis === null
      ? `always(${this.subformula})`
      : `always(${this.subformula}).within(${this.boundMillis}, "milliseconds")`;
  }
}

export class Eventually extends Formula {
  constructor(
    public boundMillis: number | null,
    public subformula: Formula,
  ) {
    super();
  }

  within(n: number, unit: TimeUnit): Formula {
    if (this.boundMillis !== null) {
      throw new Error("time bound is already set for `eventually`");
    }
    return new Eventually(durationMillis(n, unit), this.subformula);
  }

  override toString() {
    return this.boundMillis === null
      ? `eventually(${this.subformula})`
      : `eventually(${this.subformula}).within(${this.boundMillis}, "milliseconds")`;
  }
}

export class Until extends Formula {
  constructor(
    public boundMillis: number | null,
    public left: Formula,
    public right: Formula,
  ) {
    super();
  }

  within(n: number, unit: TimeUnit): Formula {
    if (this.boundMillis !== null) {
      throw new Error("time bound is already set for `until`");
    }
    return new Until(durationMillis(n, unit), this.left, this.right);
  }

  override toString() {
    return this.boundMillis === null
      ? `${this.left}.until(${this.right})`
      : `${this.left}.until(${this.right}).within(${this.boundMillis}, "milliseconds")`;
  }
}

export class Release extends Formula {
  constructor(
    public boundMillis: number | null,
    public left: Formula,
    public right: Formula,
  ) {
    super();
  }

  within(n: number, unit: TimeUnit): Formula {
    if (this.boundMillis !== null) {
      throw new Error("time bound is already set for `release`");
    }
    return new Release(durationMillis(n, unit), this.left, this.right);
  }

  override toString() {
    return this.boundMillis === null
      ? `${this.left}.release(${this.right})`
      : `${this.left}.release(${this.right}).within(${this.boundMillis}, "milliseconds")`;
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
      .replaceAll(/\t/g, "  ")
      .replaceAll(/(\|\||&&)/g, (_, operator) => "\n  " + operator);

    function liftResult(result: Formula | boolean): Formula {
      return typeof result === "boolean" ? new Pure(pretty, result) : result;
    }

    return new Thunk(pretty, () => liftResult(x()));
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
  return new ExtractorCell<T, State>(runtime, query);
}

export interface State {
  document: HTMLDocument;
  window: Window;
  navigationHistory: {
    back: NavigationEntry[];
    current: NavigationEntry;
    forward: NavigationEntry[];
  };
  errors: {
    uncaughtExceptions: {
      text: string;
      line: number;
      column: number;
      url: string | null;
      remote_object: {
        type_name: string;
        subtype: string | null;
        class_name: string | null;
        description: string | null;
        value: unknown;
      } | null;
      stacktrace:
        | { name: string; line: number; column: number; url: string }[]
        | null;
    }[];
  };
  console: ConsoleEntry[];
  lastAction: Action | null;
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
