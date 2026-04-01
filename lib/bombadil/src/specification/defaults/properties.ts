import { always, extract } from "@antithesishq/bombadil";

const responseStatus = extract((state) => {
  const first = state.window.performance.getEntriesByType("navigation")[0];
  return first && first instanceof PerformanceNavigationTiming
    ? first.responseStatus
    : null;
});

export const noHttpErrorCodes = always(
  () => (responseStatus.current ?? 0) < 400,
);

function formatException(e: {
  text: string;
  line: number;
  column: number;
  url: string | null;
  stacktrace:
    | { name: string; line: number; column: number; url: string }[]
    | null;
}): string {
  let result = e.text;
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

export const noUncaughtExceptions = always(
  () =>
    uncaughtExceptions.current.every(
      (e) =>
        !e.startsWith("Uncaught") ||
        e.startsWith("Uncaught (in promise)"),
    ),
);

export const noUnhandledPromiseRejections = always(
  () =>
    uncaughtExceptions.current.every(
      (e) => !e.startsWith("Uncaught (in promise)"),
    ),
);

const consoleErrors = extract((state) =>
  state.console.filter((e) => e.level === "error"),
);

export const noConsoleErrors = always(
  () => consoleErrors.current?.length === 0,
);
