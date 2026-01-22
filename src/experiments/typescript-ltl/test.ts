import { evaluate, step, type Residual, type ViolationTree } from "./eval";
import { Formula } from "./bombadil";
import { runtime_default, type State } from "./runtime";

export type TestResult =
  | { type: "passed" }
  | { type: "inconclusive"; residual_type: Residual["type"] }
  | { type: "failed"; violation: ViolationTree };

export function test(formula: Formula, trace: State[]): TestResult {
  if (trace.length === 0) {
    throw new Error("cant evaluate against empty trace");
  }

  runtime_default.reset();

  const time = runtime_default.register_state(trace[0]!);
  let value = evaluate(formula, time);

  for (const state of trace.slice(1)) {
    if (value.type !== "residual") {
      break;
    }
    const time = runtime_default.register_state(state);
    value = step(value.residual, time);
  }

  switch (value.type) {
    case "true":
      return { type: "passed" };
    case "false":
      return { type: "failed", violation: value.violation };
    case "residual":
      return { type: "inconclusive", residual_type: value.residual.type };
  }
}
