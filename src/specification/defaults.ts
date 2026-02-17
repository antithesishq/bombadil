import {
  always,
  extract,
  actions,
  strings,
  emails,
  integers,
  keycodes,
  type Action,
} from "@antithesishq/bombadil";

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

const content_type = extract((state) => state.document.contentType);

const can_go_back = extract(
  (state) => state.navigation_history.back.length > 0,
);

const can_go_forward_same_origin = extract((state) => {
  const entry = state.navigation_history.forward[0];
  if (!entry) return false;
  try {
    const current = new URL(state.navigation_history.current.url);
    const forward = new URL(entry.url);
    return forward.origin === current.origin;
  } catch {
    return false;
  }
});

const last_action = extract((state) => {
  const action = state.last_action;
  if (action === null) return null;
  if (typeof action === "string") return action;
  return Object.keys(action)[0] ?? null;
});

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
  if (content_type.current !== "text/html") return [];

  const scrolls: Action[] = [];

  if (!body.current) return scrolls;

  const scroll_y_max = body.current.scrollHeight - window.current.inner.height;
  const scroll_y_max_diff = scroll_y_max - window.current.scroll.y;

  if (scroll_y_max_diff >= 1) {
    scrolls.push({
      ScrollDown: {
        origin: {
          x: window.current.inner.width / 2,
          y: window.current.inner.height / 2,
        },
        distance: Math.min(window.current.inner.height / 2, scroll_y_max_diff),
      },
    });
  } else if (window.current.scroll.y > 0) {
    scrolls.push({
      ScrollUp: {
        origin: {
          x: window.current.inner.width / 2,
          y: window.current.inner.height / 2,
        },
        distance: window.current.scroll.y,
      },
    });
  }

  return scrolls;
});

// Clicks

const clickable_points = extract((state) => {
  if (!state.document.body) return [];

  const ARIA_ROLES_CLICKABLE = [
    "button",
    "link",
    "checkbox",
    "radio",
    "switch",
    "tab",
    "menuitem",
    "option",
    "treeitem",
  ];

  type ClickTarget = {
    name: string;
    content: string;
    point: { x: number; y: number };
  };
  const targets: ClickTarget[] = [];
  const added = new Set<Element>();

  function clickable_point(element: Element): { x: number; y: number } | null {
    const rect = element.getBoundingClientRect();
    if (rect.width > 0 && rect.height > 0) {
      return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    }
    return null;
  }

  function is_visible(element: Element): boolean {
    const style = state.window.getComputedStyle(element);
    return (
      style.display !== "none" &&
      style.visibility !== "hidden" &&
      parseFloat(style.opacity || "1") > 0.0
    );
  }

  function in_viewport(point: { x: number; y: number }): boolean {
    return (
      point.x >= 0 &&
      point.x <= state.window.innerWidth &&
      point.y >= 0 &&
      point.y <= state.window.innerHeight
    );
  }

  // Anchors
  const url_current = new URL(state.window.location.toString());
  for (const anchor of Array.from(state.document.body.querySelectorAll("a"))) {
    if (!(anchor instanceof HTMLAnchorElement)) continue;
    if (added.has(anchor)) continue;

    let url;
    try {
      url = new URL(anchor.href);
    } catch {
      continue;
    }

    if (anchor.target === "_blank") continue;
    if (!url.protocol.startsWith("http")) continue;
    if (!url.origin.endsWith(url_current.origin)) continue;
    if (!is_visible(anchor)) continue;

    const point = clickable_point(anchor);
    if (!point) continue;
    if (!in_viewport(point)) continue;

    targets.push({
      name: anchor.nodeName,
      content: (anchor.textContent ?? "").trim().replace(/\s+/g, " "),
      point,
    });
    added.add(anchor);
  }

  // Buttons, inputs, textareas, labels
  for (const element of Array.from(
    state.document.body.querySelectorAll("button,input,textarea,label[for]"),
  )) {
    if (added.has(element)) continue;
    // We require visibility except for input elements, which are often hidden and overlayed with custom styling.
    if (!(element instanceof HTMLInputElement) && !is_visible(element))
      continue;

    const point = clickable_point(element);
    if (!point) continue;
    if (!in_viewport(point)) continue;

    if (
      element === state.document.activeElement &&
      (element instanceof HTMLInputElement ||
        element instanceof HTMLTextAreaElement) &&
      element.value
    ) {
      continue;
    }

    targets.push({
      name: element.nodeName,
      content: (element.textContent ?? "").trim().replace(/\s+/g, " "),
      point,
    });
    added.add(element);
  }

  // ARIA role elements
  const aria_selector = ARIA_ROLES_CLICKABLE.map(
    (role) => `[role=${role}]`,
  ).join(",");
  for (const element of Array.from(
    state.document.body.querySelectorAll(aria_selector),
  )) {
    if (added.has(element)) continue;
    if (!is_visible(element)) continue;

    const point = clickable_point(element);
    if (!point) continue;
    if (!in_viewport(point)) continue;

    targets.push({
      name: element.nodeName,
      content: (element.textContent ?? "").trim().replace(/\s+/g, " "),
      point,
    });
    added.add(element);
  }

  return targets;
});

export const clicks = actions(() => {
  if (content_type.current !== "text/html") return [];
  return clickable_points.current.map(({ name, content, point }) => ({
    Click: { name, content, point },
  }));
});

// Inputs

const active_input = extract((state) => {
  const element = state.document.activeElement;
  if (!element || element === state.document.body) return null;

  if (element instanceof HTMLTextAreaElement) {
    return "textarea";
  }

  if (element instanceof HTMLInputElement) {
    return element.type;
  }

  return null;
});

export const inputs = actions(() => {
  if (content_type.current !== "text/html") return [];
  const type = active_input.current;
  if (!type) return [];

  const delay_millis = 50;

  if (type === "textarea") {
    return [{ TypeText: { text: strings().generate(), delay_millis } }];
  }

  switch (type) {
    case "text":
      return [
        { PressKey: { code: keycodes().generate() } },
        { TypeText: { text: strings().generate(), delay_millis } },
      ];
    case "email":
      return [
        { PressKey: { code: keycodes().generate() } },
        { TypeText: { text: emails().generate(), delay_millis } },
      ];
    case "number":
      return [
        { PressKey: { code: keycodes().generate() } },
        { TypeText: { text: integers().generate(), delay_millis } },
      ];
    default:
      return [];
  }
});

// Navigation

export const back = actions(() => {
  if (can_go_back.current) {
    return ["Back" as Action];
  }
  return [];
});

export const forward = actions(() => {
  if (can_go_forward_same_origin.current) {
    return ["Forward" as Action];
  }
  return [];
});

export const reload = actions(() => {
  if (last_action.current !== "Reload") {
    return ["Reload" as Action];
  }
  return [];
});
