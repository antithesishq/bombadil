import { always, Formula } from "@antithesishq/bombadil";
import { type ConsoleEntry, extract } from "@antithesishq/bombadil/browser";

const responseStatus = extract((state) => {
  const first = state.window.performance.getEntriesByType("navigation")[0];
  return first && first instanceof PerformanceNavigationTiming
    ? first.responseStatus
    : null;
});

export const noHttpErrorCodes: Formula = always(
  () => (responseStatus.current ?? 0) < 400,
);

function formatException(e: {
  text: string;
  line: number;
  column: number;
  url: string | null;
  remote_object: {
    type_name: string;
    subtype: string | null;
    class_name: string | null;
    description: string | null;
    value: unknown;
  } | null;
  stacktrace:
    | { name: string; line: number; column: number; url: string }[]
    | null;
}): string {
  let result = e.text;
  if (e.remote_object?.description) {
    const firstLine = e.remote_object.description.split("\n")[0];
    result = `${e.text} ${firstLine}`;
  } else if (
    e.remote_object?.value !== null &&
    e.remote_object?.value !== undefined
  ) {
    const value = String(e.remote_object.value);
    if (value && value !== e.text) {
      result = `${e.text} ${value}`;
    }
  }
  if (e.stacktrace) {
    for (const frame of e.stacktrace) {
      result += "\n    at ";
      if (frame.name) result += frame.name + " ";
      result += `(${frame.url}:${frame.line}:${frame.column})`;
    }
  } else if (e.url) {
    result += `\n    at ${e.url}:${e.line}:${e.column}`;
  }
  return result;
}

const uncaughtExceptions = extract((state) =>
  state.errors.uncaughtExceptions.map(formatException),
);

export const noUncaughtExceptions: Formula = always(() =>
  uncaughtExceptions.current.every(
    (e) => !e.startsWith("Uncaught") || e.startsWith("Uncaught (in promise)"),
  ),
);

export const noUnhandledPromiseRejections: Formula = always(() =>
  uncaughtExceptions.current.every(
    (e) => !e.startsWith("Uncaught (in promise)"),
  ),
);

function firstLine(text: string): string {
  const end = text.indexOf("\n");
  return end === -1 ? text : text.slice(0, end);
}

// Each string argument is cut at its first line before joining: a logged
// Error arrives as its description, which carries the stack. Other arguments
// are kept in full, as JSON.
function consoleMessage(args: ConsoleEntry["args"]): string {
  let message = "";
  for (let index = 0; index < args.length; index++) {
    const arg = args[index]!;
    if (index > 0) message += " ";
    message += typeof arg === "string" ? firstLine(arg) : JSON.stringify(arg);
  }
  return message;
}

// No timestamp or stack, so that the same errors logged in the same order
// give the same snapshot. Entries accumulate until the next navigation.
const consoleErrors = extract((state) => {
  const errors: { level: "error"; message: string }[] = [];
  for (const entry of state.console) {
    if (entry.level === "error") {
      errors.push({ level: "error", message: consoleMessage(entry.args) });
    }
  }
  return errors;
}).setStateKeyBehavior("excluded");

export const noConsoleErrors: Formula = always(
  () => consoleErrors.current?.length === 0,
);
