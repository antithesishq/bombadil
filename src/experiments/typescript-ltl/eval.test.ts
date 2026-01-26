import { describe, it, expect } from "bun:test";
import { evaluate } from "./eval";
import { ExtractorCell, condition, eventually, always } from "./bombadil";

import { Runtime } from "./runtime";
import fc, { type IProperty } from "fast-check";
import assert from "node:assert";
import { test } from "./test";

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
        const time = runtime.register_state({ left, right }, 0);
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
        const time = runtime.register_state({ left, right }, 0);
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
    length: number | fc.Arbitrary<number>,
  ): fc.Arbitrary<boolean[]> {
    length = typeof length === "number" ? fc.constant(length) : length;

    return length.chain((length) =>
      fc
        .boolean()
        .map((last) =>
          length > 1 ? [...new Array(length - 1).fill(value), last] : [last],
        ),
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
        const { runtime, pair } = test_bool_pair();
        const formula = eventually(() => pair.current.left)
          .within(5, "seconds")
          .and(eventually(() => pair.current.right).within(5, "seconds"));

        const result = test(
          runtime,
          formula,
          states.map((state, i) => ({ state, timestamp_ms: i * 1000 })),
        );
        const state_last = states[states.length - 1]!;

        switch (result.type) {
          case "failed":
            throw new Error("eventually shouldn't return false");
          case "passed": {
            expect(state_last.left || state_last.right).toBe(true);
            break;
          }
          case "inconclusive": {
            expect(result.residual.type).toMatch(/and|derived/);
          }
        }
      }),
    );
  });

  it("(always left) and (always right)", () => {
    check(
      fc.property(pairs_of_default(true), (states) => {
        const { runtime, pair } = test_bool_pair();
        const formula = always(() => pair.current.left).and(
          always(() => pair.current.right),
        );

        const result = test(
          runtime,
          formula,
          states.map((state, i) => ({ state, timestamp_ms: i * 1000 })),
        );
        const state_last = states[states.length - 1]!;

        switch (result.type) {
          case "passed":
            throw new Error("always shouldn't return true");
          case "failed": {
            expect(!state_last.left || !state_last.right).toBe(true);
            break;
          }
          case "inconclusive": {
            expect(result.residual.type).toBe("and");
          }
        }
      }),
    );
  });

  it("eventually with timeout", () => {
    check(
      fc.property(
        default_up_to(false, fc.integer({ min: 2, max: 10 })),
        (states) => {
          const runtime = new Runtime<boolean>();
          let pair = new ExtractorCell<boolean, boolean>(runtime, identity);
          const formula = eventually(() => pair.current).within(
            states.length - 1,
            "seconds",
          );

          const result = test(
            runtime,
            formula,
            states.map((state, i) => ({ state, timestamp_ms: i * 1000 })),
          );
          const state_last = states[states.length - 1]!;

          switch (result.type) {
            case "failed":
              expect(state_last).toBe(false);
              break;
            case "passed": {
              expect(state_last).toBe(true);
              break;
            }
            case "inconclusive": {
              throw new Error("eventually should terminate");
            }
          }
        },
      ),
    );
  });
});
