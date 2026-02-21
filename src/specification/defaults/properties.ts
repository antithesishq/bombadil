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

const uncaughtExceptions = extract((state) => state.errors.uncaughtExceptions);

export const noUncaughtExceptions = always(() =>
  uncaughtExceptions.current.every((e) => e.text !== "Uncaught"),
);

export const noUnhandledPromiseRejections = always(() =>
  uncaughtExceptions.current.every((e) => e.text !== "Uncaught (in promise)"),
);

const consoleErrors = extract((state) =>
  state.console.filter((e) => e.level === "error"),
);

export const noConsoleErrors = always(
  () => consoleErrors.current?.length === 0,
);
