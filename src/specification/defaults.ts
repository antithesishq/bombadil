import { always, extract, actions, type Action } from "@antithesishq/bombadil";

// Properties

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

// Actions

const body = extract((state) => {
  return state.document.body
    ? { scrollHeight: state.document.body.scrollHeight }
    : null;
});

const window = extract((state) => {
  return {
    scroll: {
      x: state.window.scrollX,
      y: state.window.scrollY,
    },
    inner: {
      width: state.window.innerWidth,
      height: state.window.innerHeight,
    },
  };
});

export const scroll = actions(() => {
  const scrolls: Action[] = [];

  if (window.current.scroll.y > 0) {
    scrolls.push({
      ScrollUp: {
        origin: {
          x: window.current.inner.width / 2,
          y: window.current.inner.height / 2,
        },
        distance: Math.min(
          window.current.inner.height / 2,
          window.current.scroll.y,
        ),
      },
    });
  }

  if (body.current) {
    const scroll_y_max =
      body.current.scrollHeight - window.current.inner.height;
    const scroll_y_max_diff = scroll_y_max - window.current.scroll.y;
    if (scroll_y_max_diff >= 1) {
      scrolls.push({
        ScrollDown: {
          origin: {
            x: window.current.inner.width / 2,
            y: window.current.inner.height / 2,
          },
          distance: Math.min(
            window.current.inner.height / 2,
            scroll_y_max_diff,
          ),
        },
      });
    }
  }

  return scrolls;
});
