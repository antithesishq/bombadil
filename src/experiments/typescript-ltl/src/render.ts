import type { ViolationTree } from "./eval";

export function render_violation(violation: ViolationTree): string {
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
        return indent(indent_level, `!(${violation.condition})`);
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
        return `${indent(indent_level, `as of ${violation.start.valueOf()}ms, it should always be the case that`)}\n\n${indent(indent_level + 1, violation.formula.toString())}\n\n${indent(indent_level, `but at ${violation.time.valueOf()}ms`)}\n\n${inner(indent_level + 1, violation.violation)}`;
    }
  }

  return inner(0, violation);
}
