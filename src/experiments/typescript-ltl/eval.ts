import {
  Always,
  And,
  Contextful,
  Eventually,
  Formula,
  Implies,
  Next,
  Or,
  Pure,
} from "./bombadil";
import type { Time } from "./time";

export type Value =
  | { type: "true" }
  | { type: "false"; violation: ViolationTree }
  | { type: "residual"; residual: Residual };

export type Residual =
  | { type: "true" }
  | { type: "false"; violation: ViolationTree }
  | { type: "derived"; derived: DerivedFormula }
  | { type: "and"; left: Residual; right: Residual }
  | { type: "or"; left: Residual; right: Residual }
  | {
      type: "implies";
      antecedent_formula: Formula;
      antecedent: Residual;
      consequent: Residual;
    }
  | {
      type: "and_always";
      formula: Formula;
      start: Time;
      left: Residual;
      right: Residual;
    }
  | {
      type: "or_eventually";
      subformula: Formula;
      start: Time;
      deadline: Time;
      left: Residual;
      right: Residual;
    };

export type DerivedFormula =
  | { type: "next"; start: Time; formula: Formula }
  | { type: "always"; start: Time; formula: Formula }
  | { type: "eventually"; start: Time; deadline: Time; formula: Formula };

export type ViolationTree =
  | { type: "false"; time: Time; condition: string }
  | { type: "next"; time: Time; formula: Formula }
  | {
      type: "always";
      start: Time;
      time: Time;
      formula: Formula;
      violation: ViolationTree;
    }
  | { type: "eventually"; time: Time; formula: Formula }
  | { type: "and"; left: ViolationTree; right: ViolationTree }
  | { type: "or"; left: ViolationTree; right: ViolationTree }
  | { type: "implies"; antecedent: Formula; consequent: ViolationTree };

export function evaluate(formula: Formula, time: Time): Value {
  switch (true) {
    case formula instanceof Pure:
      return formula.value
        ? { type: "true" }
        : {
            type: "false",
            violation: { type: "false", time, condition: formula.toString() },
          };
    case formula instanceof Contextful:
      return evaluate(formula.apply(), time);
    case formula instanceof And: {
      const left = evaluate(formula.left, time);
      const right = evaluate(formula.right, time);
      return evaluate_and(left, right);
    }
    case formula instanceof Or: {
      const left = evaluate(formula.left, time);
      const right = evaluate(formula.right, time);
      return evaluate_or(left, right);
    }
    case formula instanceof Implies: {
      const antecedent = evaluate(formula.antecedent, time);
      const consequent = evaluate(formula.consequent, time);
      return evaluate_implies(formula.antecedent, antecedent, consequent);
    }
    case formula instanceof Next:
      return {
        type: "residual",
        residual: {
          type: "derived",
          derived: { type: "next", start: time, formula: formula.subformula },
        },
      };
    case formula instanceof Eventually:
      return evaluate_eventually(
        formula.subformula,
        time,
        time.plus(formula.timeout),
        time,
      );
    case formula instanceof Always:
      return evaluate_always(formula.subformula, time, time);
    default:
      throw new Error(`unsupported formula: ${formula}`);
  }
}

function evaluate_and(left: Value, right: Value): Value {
  switch (left.type) {
    case "true":
      return right;
    case "false": {
      switch (right.type) {
        case "true":
          return left;
        case "false":
          return {
            type: "false",
            violation: {
              type: "and",
              left: left.violation,
              right: right.violation,
            },
          };
        case "residual":
          // NOTE: We short-circuit "in time" by only considering the current
          // false operand. Could also await both of them and thus collect two
          // "false" operands when both are available.
          return left;
      }
    }
    case "residual": {
      switch (right.type) {
        case "true":
          return left;
        case "false":
          // Same here regarding short-circuiting.
          return right;
        case "residual":
          return {
            type: "residual",
            residual: {
              type: "and",
              left: left.residual,
              right: right.residual,
            },
          };
      }
    }
  }
}

function evaluate_or(left: Value, right: Value): Value {
  switch (left.type) {
    case "true":
      // NOTE: We short-circuit "in time" by only considering the current
      // true operand. Could also await both of them and thus collect two
      // "false" operands when both are available.
      return left;
    case "false": {
      switch (right.type) {
        case "true":
          return right;
        case "false":
          return {
            type: "false",
            violation: {
              type: "or",
              left: left.violation,
              right: right.violation,
            },
          };
        case "residual":
          return right;
      }
    }
    case "residual": {
      switch (right.type) {
        case "true":
          // Same here regarding short-circuiting.
          return right;
        case "false":
          return left;
        case "residual":
          return {
            type: "residual",
            residual: {
              type: "or",
              left: left.residual,
              right: right.residual,
            },
          };
      }
    }
  }
}

function evaluate_implies(
  antecedent_formula: Formula,
  antecedent: Value,
  consequent: Value,
): Value {
  switch (antecedent.type) {
    case "false":
      return { type: "true" };
    case "true": {
      switch (consequent.type) {
        case "false":
          return {
            type: "false",
            violation: {
              type: "implies",
              antecedent: antecedent_formula,
              consequent: consequent.violation,
            },
          };
        case "true":
          return { type: "true" };
        case "residual":
          return {
            type: "residual",
            residual: {
              type: "implies",
              antecedent_formula: antecedent_formula,
              antecedent: antecedent,
              consequent: consequent.residual,
            },
          };
      }
    }
    case "residual": {
      switch (consequent.type) {
        case "true":
          return consequent;
        case "false":
          return {
            type: "residual",
            residual: {
              type: "implies",
              antecedent_formula: antecedent_formula,
              antecedent: antecedent.residual,
              consequent: { type: "false", violation: consequent.violation },
            },
          };
        case "residual":
          return {
            type: "residual",
            residual: {
              type: "implies",
              antecedent_formula,
              antecedent: antecedent.residual,
              consequent: consequent.residual,
            },
          };
      }
    }
  }
}

function evaluate_eventually(
  subformula: Formula,
  start: Time,
  deadline: Time,
  time: Time,
): Value {
  if (deadline.is_before(time)) {
    return {
      type: "false",
      violation: { type: "eventually", formula: subformula, time },
    };
  }

  const residual: Residual = {
    type: "derived",
    derived: {
      type: "eventually",
      formula: subformula,
      start,
      deadline,
    },
  };

  const value = evaluate(subformula, time);
  switch (value.type) {
    case "true":
      return value;
    case "false":
      return { type: "residual", residual };
    case "residual":
      return {
        type: "residual",
        residual: {
          type: "or_eventually",
          subformula,
          start,
          deadline,
          left: value.residual,
          right: residual,
        },
      };
  }
}

function evaluate_or_eventually(
  start: Time,
  deadline: Time,
  subformula: Formula,
  time: Time,
  left: Value,
  right: Value,
): Value {
  if (deadline.is_before(time)) {
    return {
      type: "false",
      violation: { type: "eventually", formula: subformula, time },
    };
  }

  switch (left.type) {
    case "true":
      return left;
    case "false": {
      switch (right.type) {
        case "true":
          return right;
        case "false":
          return {
            type: "false",
            // NOTE: We ignore the left-side violation in `eventually` in
            // order to not build up a giant violation tree of all the
            // non-evidence we've seen (e.g. X was not true in state 1 and
            // X was not true in state 2 and ...).
            violation: right.violation,
          };
        case "residual":
          return right;
      }
    }
    case "residual": {
      switch (right.type) {
        case "true":
          return right;
        case "false":
          return left;
        case "residual":
          return {
            type: "residual",
            residual: {
              type: "or_eventually",
              subformula,
              start,
              deadline,
              left: left.residual,
              right: right.residual,
            },
          };
      }
    }
  }
}

function evaluate_always(subformula: Formula, start: Time, time: Time): Value {
  const residual: Residual = {
    type: "derived",
    derived: {
      type: "always",
      formula: subformula,
      start,
    },
  };
  const value = evaluate(subformula, time);
  switch (value.type) {
    case "true":
      return { type: "residual", residual };
    case "false":
      return {
        type: "false",
        violation: {
          type: "always",
          violation: value.violation,
          formula: subformula,
          start,
          time,
        },
      };
    case "residual":
      return {
        type: "residual",
        residual: {
          type: "and_always",
          formula: subformula,
          left: value.residual,
          right: residual,
          start,
        },
      };
  }
}

function evaluate_and_always(
  formula: Formula,
  start: Time,
  time: Time,
  left: Value,
  right: Value,
): Value {
  switch (left.type) {
    case "true":
      return right;
    case "false": {
      switch (right.type) {
        case "true":
          return {
            type: "false",
            violation: {
              type: "always",
              violation: left.violation,
              formula,
              start,
              time,
            },
          };
        case "false":
          return {
            type: "false",
            violation: {
              type: "always",
              violation: {
                type: "and",
                left: left.violation,
                right: right.violation,
              },
              formula,
              start,
              time,
            },
          };
        case "residual":
          // NOTE: We short-circuit "in time" by only considering the current
          // false operand. Could also await both of them and thus collect two
          // "false" operands when both are available.
          return left;
      }
    }
    case "residual": {
      switch (right.type) {
        case "true":
          return left;
        case "false":
          // Same here regarding short-circuiting.
          return right;
        case "residual":
          return {
            type: "residual",
            residual: {
              type: "and",
              left: left.residual,
              right: right.residual,
            },
          };
      }
    }
  }
}

export function step(residual: Residual, time: Time): Value {
  switch (residual.type) {
    case "true":
      return { type: "true" };
    case "false":
      return { type: "false", violation: residual.violation };
    case "and":
      return evaluate_and(
        step(residual.left, time),
        step(residual.right, time),
      );
    case "or":
      return evaluate_or(step(residual.left, time), step(residual.right, time));
    case "implies":
      return evaluate_implies(
        residual.antecedent_formula,
        step(residual.antecedent, time),
        step(residual.consequent, time),
      );
    case "derived":
      switch (residual.derived.type) {
        case "next":
          return evaluate(residual.derived.formula, time);
        case "always":
          return evaluate_always(
            residual.derived.formula,
            residual.derived.start,
            time,
          );
        case "eventually":
          return evaluate_eventually(
            residual.derived.formula,
            residual.derived.start,
            residual.derived.deadline,
            time,
          );
      }
    case "and_always":
      return evaluate_and_always(
        residual.formula,
        residual.start,
        time,
        step(residual.left, time),
        step(residual.right, time),
      );
    case "or_eventually":
      return evaluate_or_eventually(
        residual.start,
        residual.deadline,
        residual.subformula,
        time,
        step(residual.left, time),
        step(residual.right, time),
      );
  }
}
