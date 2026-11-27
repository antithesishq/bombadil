import { type Cell } from "@antithesishq/bombadil";
import { type ConsoleEntry, extract } from "@antithesishq/bombadil/browser";

function firstLine(text: string): string {
  const end = text.indexOf("\n");
  return end === -1 ? text : text.slice(0, end);
}

function consoleMessage(args: ConsoleEntry["args"]): string {
  let message = "";
  for (let index = 0; index < args.length; index++) {
    const arg = args[index]!;
    if (index > 0) message += " ";
    message += typeof arg === "string" ? firstLine(arg) : JSON.stringify(arg);
  }
  return message;
}

/**
 * The `console.error` calls since the previous action, with whole arguments
 * and stack traces.
 */
export const consoleErrorEntries: Cell<ConsoleEntry[]> = extract((state) =>
  state.console.filter((entry) => entry.level === "error"),
);

/**
 * The abstraction of `consoleErrorEntries` that `noConsoleErrors` reads: no
 * timestamps, no stacks, and only the first line of each string argument.
 */
export const consoleErrors: Cell<{ level: "error"; message: string }[]> =
  consoleErrorEntries.map((entries) =>
    entries.map((entry) => ({
      level: "error",
      message: consoleMessage(entry.args),
    })),
  );
