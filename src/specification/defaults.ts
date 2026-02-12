import { always, extract } from "@antithesishq/bombadil";

const response_status = extract((state) => {
  const first = state.window.performance.getEntriesByType("navigation")[0];
  return first && first instanceof PerformanceNavigationTiming
    ? first.responseStatus
    : null;
});

export const no_http_error_codes = always(
  () => (response_status.current ?? 0) < 400,
);

const uncaught_exceptions = extract(
  (state) => state.errors.uncaught_exceptions,
);

export const no_uncaught_exceptions = always(() =>
  uncaught_exceptions.current.every((e) => e.text !== "Uncaught"),
);

export const no_unhandled_promise_rejections = always(() =>
  uncaught_exceptions.current.every((e) => e.text !== "Uncaught (in promise)"),
);

const console_errors = extract((state) =>
  state.console.filter((e) => e.level === "error"),
);

export const no_console_errors = always(
  () => console_errors.current?.length === 0,
);
