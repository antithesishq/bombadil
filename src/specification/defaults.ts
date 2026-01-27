import { always, extract } from "bombadil";

const response_status = extract((state) => {
  const first = state.window.performance.getEntriesByType("navigation")[0];
  return first && first instanceof PerformanceNavigationTiming
    ? first.responseStatus
    : null;
});

export const no_http_error_codes = always(
  () => (response_status.current ?? 0) < 400,
);

const uncaught_exception = extract((state) => state.errors.uncaught_exception);

export const no_uncaught_exceptions = always(
  () => uncaught_exception.current === null,
);

const unhandled_promise_rejection = extract(
  (state) => state.errors.unhandled_promise_rejection,
);

export const no_unhandled_promise_rejections = always(
  () => unhandled_promise_rejection.current === null,
);

const console_errors = extract((state) =>
  state.console.filter((e) => e.level === "error"),
);

export const no_console_errors = always(
  () => console_errors.current?.length === 0,
);
