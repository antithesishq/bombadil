import {
  ExtractorCell,
  Runtime,
  type Cell,
  type JSON,
  type TimeUnit,
} from "@antithesishq/bombadil/internal";
// `ActionGenerator` is imported as a type only — re-exported below as a
// value. Importing the value AND re-exporting it produces a duplicate
// `const { ActionGenerator } = require(...)` binding after bundling,
// which throws SyntaxError at eval time and silently unsets
// __bombadilRequire. The local code only uses it for return-type
// annotations, so type-import is enough.
import {
  makeActions,
  makeWeighted,
  type ActionGenerator,
  type Tree,
} from "@antithesishq/bombadil/actions";

export { type Cell, type JSON } from "@antithesishq/bombadil/internal";
export {
  ActionGenerator,
  type Tree,
  type Generator,
  from,
  strings,
  emails,
  integers,
  keycodes,
  randomRange,
} from "@antithesishq/bombadil/actions";

/**
 * The runtime singleton that all `extract` calls register into and that
 * the Rust verifier reads `runExtractors` from. JS erases the `<S>`
 * parameter, so a single instance shared between drivers is fine —
 * specs target one interface at a time, and driver modules narrow the
 * type at their `extract` re-export.
 *
 * @internal
 */
export const runtime = new Runtime<unknown>();

/**
 * Untyped extract factory. Driver modules wrap this with a `State`-bound
 * signature so specs can write `extract((state) => state.foo)` and have
 * `state` inferred.
 *
 * @internal
 */
export function extract<S, T extends JSON>(
  query: (state: S) => T,
): Cell<T> {
  return new ExtractorCell<T, unknown>(
    runtime,
    query as (state: unknown) => T,
  );
}

/**
 * Untyped actions factory. Driver modules wrap this with their `Action`
 * union bound so specs get type-checked array literals.
 *
 * @internal
 */
export function actions<A>(
  generate: () => Tree<A> | A[],
): ActionGenerator<A> {
  return makeActions(generate);
}

/**
 * Untyped weighted factory. Driver modules wrap this with their `Action`
 * union bound.
 *
 * @internal
 */
export function weighted<A>(
  value: [number, A | ActionGenerator<A>][],
): ActionGenerator<A> {
  return makeWeighted(value);
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
    public boundMillis: number | null,
    public subformula: Formula,
  ) {
    super();
  }

  within(n: number, unit: TimeUnit): Formula {
    if (this.boundMillis !== null) {
      throw new Error("time bound is already set for `always`");
    }
    let durationMillis: number;
    switch (unit) {
      case "milliseconds":
        durationMillis = n;
        break;
      case "seconds":
        durationMillis = n * 1000;
        break;
    }
    return new Always(durationMillis, this.subformula);
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
    let durationMillis: number;
    switch (unit) {
      case "milliseconds":
        durationMillis = n;
        break;
      case "seconds":
        durationMillis = n * 1000;
        break;
    }
    return new Eventually(durationMillis, this.subformula);
  }

  override toString() {
    return this.boundMillis === null
      ? `eventually(${this.subformula})`
      : `eventually(${this.subformula}).within(${this.boundMillis}, "milliseconds")`;
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
