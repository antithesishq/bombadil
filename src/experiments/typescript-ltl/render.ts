import type { ViolationTree } from "./eval";

export function render_violation(violation: ViolationTree): string {
  console.log(violation);
  function indent(level: number, text: string) {
    const prefix = "  ".repeat(level);
    return text
      .split("\n")
      .map((line) => prefix + line)
      .join("\n");
  }

  function inner(indent_level: number, violation: ViolationTree): string {
    switch (violation.type) {
      case "false":
        return violation.condition;
      case "and":
        return `${render_violation(violation.left)} and ${render_violation(violation.right)}`;
      case "or":
        return `${render_violation(violation.left)} or ${render_violation(violation.right)}`;
      case "implies":
        return `${inner(indent_level + 1, violation.consequent)}\n\n${indent(indent_level, "which was implied by")}\n\n${indent(indent_level + 1, violation.antecedent.toString())} `;
      case "next":
        return `${violation.formula} at ${violation.time.valueOf()}ms`;
      case "eventually":
        return `${indent(indent_level + 1, violation.formula.toString())}\n\n${indent(indent_level, `wasn't observed and timed out at ${violation.time.valueOf()}ms`)}`;
      case "always":
        return `${violation.formula.toString()} ${violation.violation} at ${violation.time.valueOf()}ms`;
    }
  }

  return inner(0, violation);
}
