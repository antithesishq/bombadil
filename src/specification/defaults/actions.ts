import {
  actions,
  weighted,
  extract,
  strings,
  emails,
  integers,
  keycodes,
  type Action,
} from "@antithesishq/bombadil";

const contentType = extract((state) => state.document.contentType);

const canGoBack = extract((state) => state.navigationHistory.back.length > 0);

const canGoForwardSameOrigin = extract((state) => {
  const entry = state.navigationHistory.forward[0];
  if (!entry) return false;
  try {
    const current = new URL(state.navigationHistory.current.url);
    const forward = new URL(entry.url);
    return forward.origin === current.origin;
  } catch {
    return false;
  }
});

const lastAction = extract((state) => {
  const action = state.lastAction;
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
  if (contentType.current !== "text/html") return [];

  if (!body.current) return [];

  const scrollYMax = body.current.scrollHeight - window.current.inner.height;
  const scrollYMaxDiff = scrollYMax - window.current.scroll.y;

  if (scrollYMaxDiff >= 1) {
    return [
      {
        ScrollDown: {
          origin: {
            x: window.current.inner.width / 2,
            y: window.current.inner.height / 2,
          },
          distance: Math.min(window.current.inner.height / 2, scrollYMaxDiff),
        },
      } as Action,
    ];
  } else if (window.current.scroll.y > 0) {
    return [
      {
        ScrollUp: {
          origin: {
            x: window.current.inner.width / 2,
            y: window.current.inner.height / 2,
          },
          distance: window.current.scroll.y,
        },
      } as Action,
    ];
  }

  return [];
});

// Clicks

const clickablePoints = extract((state) => {
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

  function clickablePoint(element: Element): { x: number; y: number } | null {
    const rect = element.getBoundingClientRect();
    if (rect.width > 0 && rect.height > 0) {
      return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    }
    return null;
  }

  function isVisible(element: Element): boolean {
    const style = state.window.getComputedStyle(element);
    return (
      style.display !== "none" &&
      style.visibility !== "hidden" &&
      parseFloat(style.opacity || "1") > 0.0
    );
  }

  function inViewport(point: { x: number; y: number }): boolean {
    return (
      point.x >= 0 &&
      point.x <= state.window.innerWidth &&
      point.y >= 0 &&
      point.y <= state.window.innerHeight
    );
  }

  // Like querySelectorAll, but searches recursively into shadow roots and iframes.
  //
  // TODO: make this a part of the bombadil package so that others can use it (depends
  // on https://github.com/antithesishq/bombadil/issues/17)
  function queryAll(root: Element, selector: string): Element[] {
    const queue: Element[] = [root];
    const results: Element[] = [];
    while (queue.length > 0) {
      const element = queue.pop()!;
      if (element.matches(selector)) {
        results.push(element);
      }
      if (element.shadowRoot) {
        for (const child of Array.from(element.shadowRoot.children)) {
          queue.push(child);
        }
      } else if (
        element instanceof HTMLIFrameElement &&
        element.contentDocument &&
        element.contentDocument.body
      ) {
        queue.push(element.contentDocument.body);
      } else {
        for (const child of Array.from(element.children)) {
          queue.push(child);
        }
      }
    }
    return results;
  }

  // Anchors
  const urlCurrent = new URL(state.window.location.toString());
  for (const anchor of queryAll(state.document.body, "a")) {
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
    if (url.hostname !== urlCurrent.hostname) continue;
    if (url.port !== "" && url.port !== urlCurrent.port) continue;
    if (!isVisible(anchor)) continue;

    const point = clickablePoint(anchor);
    if (!point) continue;
    if (!inViewport(point)) continue;

    targets.push({
      name: anchor.nodeName,
      content: (anchor.textContent ?? "").trim().replace(/\s+/g, " "),
      point,
    });
    added.add(anchor);
  }

  // Buttons, inputs, textareas, labels
  for (const element of queryAll(
    state.document.body,
    "button,input,textarea,label[for]",
  )) {
    if (added.has(element)) continue;
    // We require visibility except for input elements, which are often hidden and overlayed with custom styling.
    if (!(element instanceof HTMLInputElement) && !isVisible(element)) continue;

    const point = clickablePoint(element);
    if (!point) continue;
    if (!inViewport(point)) continue;

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
  const ariaSelector = ARIA_ROLES_CLICKABLE.map(
    (role) => `[role=${role}]`,
  ).join(",");
  for (const element of queryAll(state.document.body, ariaSelector)) {
    if (added.has(element)) continue;
    if (!isVisible(element)) continue;

    const point = clickablePoint(element);
    if (!point) continue;
    if (!inViewport(point)) continue;

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
  if (contentType.current !== "text/html") return [];
  return clickablePoints.current.map(
    ({ name, content, point }) =>
      ({
        Click: { name, content, point },
      }) as Action,
  );
});

// Inputs

const activeInput = extract((state) => {
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
  if (contentType.current !== "text/html") return [];
  const type = activeInput.current;
  if (!type) return [];

  const delayMillis = integers().min(1).max(100).generate();

  if (type === "textarea") {
    return weighted([
      [1, { PressKey: { code: keycodes().generate() } }],
      [3, { TypeText: { text: strings().minSize(1).generate(), delayMillis } }],
    ]).generate();
  }

  switch (type) {
    case "text":
      return weighted([
        [1, { PressKey: { code: keycodes().generate() } }],
        [
          3,
          { TypeText: { text: strings().minSize(1).generate(), delayMillis } },
        ],
      ]).generate();
    case "email":
      return weighted([
        [1, { PressKey: { code: keycodes().generate() } }],
        [3, { TypeText: { text: emails().generate(), delayMillis } }],
      ]).generate();
    case "number":
      return weighted([
        [1, { PressKey: { code: keycodes().generate() } }],
        [
          3,
          {
            TypeText: {
              text: integers().min(0).max(10000).generate().toString(),
              delayMillis,
            },
          },
        ],
      ]).generate();
    default:
      return [];
  }
});

// Navigation

export const back = actions(() => {
  if (canGoBack.current) return ["Back" as Action];
  return [];
});

export const forward = actions(() => {
  if (canGoForwardSameOrigin.current) return ["Forward" as Action];
  return [];
});

export const reload = actions(() => {
  if (lastAction.current !== "Reload") return ["Reload" as Action];
  return [];
});

export const navigation = weighted([
  [10, back],
  [1, forward],
  [1, reload],
]);
