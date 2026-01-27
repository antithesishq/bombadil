import { evaluate, step, type Residual, type ViolationTree } from "./eval";
import { Formula } from "./bombadil";
import { Runtime } from "./runtime";

export type TestResult =
  | { type: "passed" }
  | { type: "inconclusive"; residual: Residual }
  | { type: "failed"; violation: ViolationTree };

export function test<S>(
  runtime: Runtime<S>,
  formula: Formula,
  trace: { state: S; timestamp_ms: number }[],
): TestResult {
  if (trace.length === 0) {
    throw new Error("cant evaluate against empty trace");
  }

  const time = runtime.register_state(trace[0]!.state, trace[0]!.timestamp_ms);
  let value = evaluate(formula, time);

  for (const entry of trace.slice(1)) {
    if (value.type !== "residual") {
      break;
    }
    const time = runtime.register_state(entry.state, entry.timestamp_ms);
    value = step(value.residual, time);
  }

  switch (value.type) {
    case "true":
      return { type: "passed" };
    case "false":
      return { type: "failed", violation: value.violation };
    case "residual":
      return { type: "inconclusive", residual: value.residual };
  }
}
