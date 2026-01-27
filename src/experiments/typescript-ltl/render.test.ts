import { describe, expect, it } from "bun:test";
import { test } from "./test";
import { always, condition, eventually, next } from "./bombadil";
import { ExtractorCell, Runtime } from "./runtime";
import { render_violation } from "./render";
import assert from "node:assert";

describe("render_violation", () => {
  it("always(x and y)", async () => {
    type TestState = {
      x: number;
      y: number;
    };
    const trace = [
      {
        state: {
          x: 0,
          y: 0,
        } satisfies TestState,
        timestamp_ms: 0,
      },
      {
        state: {
          x: 0,
          y: 0,
        } satisfies TestState,
        timestamp_ms: 100,
      },
      {
        state: {
          x: 0,
          y: 6,
        } satisfies TestState,
        timestamp_ms: 4000,
      },
    ];
    const runtime = new Runtime<TestState>();
    const state = new ExtractorCell<TestState, TestState>(
      runtime,
      (state) => state,
    );

    const formula = always(
      condition(() => state.current.x < 10).and(() => state.current.y < 5),
    );

    const result = test(runtime, formula, trace);
    expect(result.type).toBe("failed");
    assert(result.type === "failed");
    expect(render_violation(result.violation)).toContain(
      "(state.current.x < 10) && (state.current.y < 5)",
    );
    expect(render_violation(result.violation)).toContain(
      "!(state.current.y < 5)",
    );
  });
});

describe("render_violation", () => {
  it("eventually(x and y)", async () => {
    type TestState = {
      x: number;
      y: number;
    };
    const trace = [
      {
        state: {
          x: 0,
          y: 0,
        } satisfies TestState,
        timestamp_ms: 0,
      },
      {
        state: {
          x: 0,
          y: 0,
        } satisfies TestState,
        timestamp_ms: 100,
      },
      {
        state: {
          x: 0,
          y: 6,
        } satisfies TestState,
        timestamp_ms: 4000,
      },
    ];
    const runtime = new Runtime<TestState>();
    const state = new ExtractorCell<TestState, TestState>(
      runtime,
      (state) => state,
    );

    const formula = eventually(
      condition(() => state.current.x > 10).and(() => state.current.y > 5),
    ).within(3, "seconds");

    const result = test(runtime, formula, trace);
    expect(result.type).toBe("failed");
    assert(result.type === "failed");
    expect(render_violation(result.violation)).toContain(
      "(state.current.x > 10) && (state.current.y > 5)",
    );
    // TODO: timed out at DEADLINE, not the state's time.
    expect(render_violation(result.violation)).toContain("timed out at 4000ms");
  });
});
