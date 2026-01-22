import { describe, it, expect } from "bun:test";
import { evaluate, step } from "./eval";
import { ExtractorCell, condition, eventually, always } from "./bombadil";

import { Runtime } from "./runtime";
import fc, { type IProperty } from "fast-check";
import assert from "node:assert";

function check(property: IProperty<any>) {
  try {
    fc.assert(property);
  } catch (e) {
    if (!!e && e instanceof Error) {
      // We have to unwrap the underlying fast-check error here to get actual useful
      // output on property test failures.
      if (!!e.cause) {
        throw new Error(`${e.message}\n\n${e.cause.toString()}`);
      } else {
        throw new Error(`${e.message}`);
      }
    } else {
      throw e;
    }
  }
}

function identity<T>(x: T): T {
  return x;
}

type Pair<T> = { left: T; right: T };

describe("eval", () => {
  function test_bool_pair() {
    const runtime = new Runtime<Pair<boolean>>();
    let pair = new ExtractorCell<Pair<boolean>, Pair<boolean>>(
      runtime,
      identity,
    );
    return { runtime, pair };
  }

  it("and", () => {
    check(
      fc.property(fc.tuple(fc.boolean(), fc.boolean()), ([left, right]) => {
        const { runtime, pair } = test_bool_pair();
        const formula = condition(() => pair.current.left).and(
          () => pair.current.right,
        );
        const time = runtime.register_state({ left, right });
        const value = evaluate(formula, time);
        const type_expected = left && right ? "true" : "false";
        expect(value.type).toEqual(type_expected);
        if (!left && !right) {
          assert.ok(value.type === "false");
          expect(value.violation.type).toEqual("and");
        }
      }),
    );
  });

  it("or", () => {
    check(
      fc.property(fc.tuple(fc.boolean(), fc.boolean()), ([left, right]) => {
        const { runtime, pair } = test_bool_pair();
        const formula = condition(() => pair.current.left).or(
          () => pair.current.right,
        );
        const time = runtime.register_state({ left, right });
        const value = evaluate(formula, time);
        const type_expected = left || right ? "true" : "false";
        expect(value.type).toEqual(type_expected);
        if (!(left || right)) {
          assert(value.type === "false");
          expect(value.violation.type).toEqual("or");
        }
      }),
    );
  });

  function default_up_to(
    value: boolean,
    length: number,
  ): fc.Arbitrary<boolean[]> {
    if (length <= 0) {
      throw new Error("default_up_to length must be >= 1");
    }
    return fc
      .boolean()
      .map((last) =>
        length > 1 ? [...new Array(length - 1).fill(value), last] : [last],
      );
  }

  function zip_pairs<T>(left: T[], right: T[]): Pair<T>[] {
    const pairs: { left: T; right: T }[] = [];
    for (let i = 0; i < Math.min(left.length, right.length); i++) {
      pairs.push({ left: left[i]!, right: right[i]! });
    }
    return pairs;
  }

  function pairs_of_default(value: boolean): fc.Arbitrary<Pair<boolean>[]> {
    return fc.noShrink(
      fc.integer({ min: 1, max: 3 }).chain((length) => {
        return fc
          .tuple(default_up_to(value, length), default_up_to(value, length))
          .map(([left, right]) => zip_pairs(left, right));
      }),
    );
  }

  it("(eventually left) and (eventually right)", () => {
    check(
      fc.property(pairs_of_default(false), (states) => {
        expect(states).not.toBeEmpty();
        const { runtime, pair } = test_bool_pair();
        const formula = eventually(() => pair.current.left)
          .within(5, "seconds")
          .and(eventually(() => pair.current.right).within(5, "seconds"));

        let state_last = states.shift()!;
        const time = runtime.register_state(state_last);
        let value = evaluate(formula, time);

        while (states.length > 0) {
          if (value.type !== "residual") {
            break;
          }
          state_last = states.shift()!;
          const time = runtime.register_state(state_last);
          value = step(value.residual, time);
        }

        const type_expected =
          state_last.left && state_last.right ? "true" : "residual";

        expect(value.type).toEqual(type_expected);

        switch (value.type) {
          case "false":
            throw new Error("eventually shouldn't return false");
          case "true": {
            expect(state_last.left || state_last.right).toBe(true);
            break;
          }
          case "residual": {
            expect(value.residual.type).toMatch(/and|derived/);
          }
        }
      }),
    );
  });

  it("(always left) and (always right)", () => {
    check(
      fc.property(pairs_of_default(true), (states) => {
        expect(states).not.toBeEmpty();
        const { runtime, pair } = test_bool_pair();
        const formula = always(() => pair.current.left).and(
          always(() => pair.current.right),
        );

        let state_last = states.shift()!;
        const time = runtime.register_state(state_last);
        let value = evaluate(formula, time);

        while (states.length > 0) {
          if (value.type !== "residual") {
            break;
          }
          state_last = states.shift()!;
          const time = runtime.register_state(state_last);
          value = step(value.residual, time);
        }

        switch (value.type) {
          case "true":
            throw new Error("always shouldn't return true");
          case "false": {
            expect(!state_last.left || !state_last.right).toBe(true);
            break;
          }
          case "residual": {
            expect(value.residual.type).toBe("and");
          }
        }
      }),
    );
  });
});
