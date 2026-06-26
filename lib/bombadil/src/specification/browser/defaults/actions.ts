import type { Cell } from "@antithesishq/bombadil";
import type { Range } from "@antithesishq/bombadil/actions";
import {
  actions,
  weighted,
  extract,
  type Action,
  Fingerprint,
  getFingerprint,
} from "@antithesishq/bombadil/browser";
import {
  clickablePoint,
  inViewport,
  isVisible,
  queryAll,
} from "@antithesishq/bombadil/browser/dom";

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

export const lastAction: Cell<Action | null> = extract(
  (state) => state.lastAction,
);

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

export const waitOnce = actions(() => {
  if (lastAction.current !== "Wait") {
    return ["Wait"];
  } else {
    return [];
  }
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
      },
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
      },
    ];
  }

  return [];
});

// Clicks

type ClickTarget = {
  fingerprint: Fingerprint;
  point: { x: number; y: number };
};

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

  const FORM_CONTROL_TAGS = ["button", "input", "textarea"];

  const targets: ClickTarget[] = [];
  const added = new Set<Element>();

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
    if (!isVisible(state.window, anchor)) continue;

    const point = clickablePoint(anchor);
    if (!point) continue;
    if (!inViewport(state.window, point)) continue;

    targets.push({
      fingerprint: getFingerprint(anchor),
      point,
    });
    added.add(anchor);
  }

  // Buttons, inputs, textareas, labels
  const formControlsSelector = FORM_CONTROL_TAGS.map(
    (tag) => `${tag}:not(:disabled)`,
  ).join(",");
  for (const element of queryAll(
    state.document.body,
    `${formControlsSelector},label[for]`,
  )) {
    if (added.has(element)) continue;
    // We require visibility except for input elements, which are often hidden and overlayed with custom styling.
    if (
      !(element instanceof HTMLInputElement) &&
      !isVisible(state.window, element)
    )
      continue;

    if (element instanceof HTMLInputElement && element.type === "file") {
      continue;
    }

    if (element instanceof HTMLLabelElement) {
      const control = element.control;
      if (control && control.matches(":disabled")) continue;
    }

    const point = clickablePoint(element);
    if (!point) continue;
    if (!inViewport(state.window, point)) continue;

    if (
      element === state.document.activeElement &&
      (element instanceof HTMLInputElement ||
        element instanceof HTMLTextAreaElement) &&
      element.value
    ) {
      continue;
    }

    targets.push({
      fingerprint: getFingerprint(element),
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
    if (!isVisible(state.window, element)) continue;

    const point = clickablePoint(element);
    if (!point) continue;
    if (!inViewport(state.window, point)) continue;

    targets.push({
      fingerprint: getFingerprint(element),
      point,
    });
    added.add(element);
  }

  return targets;
});

export const clicks = actions(() => {
  if (contentType.current !== "text/html") return [];
  return (clickablePoints.current ?? []).map(({ fingerprint, point }) => ({
    Click: { fingerprint, point },
  }));
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

  if (type === "file") return [];

  const delayMillis: Range = [1, 100];

  const keycodes = weighted([
    [1, { PressKey: { code: 8 } }],
    [1, { PressKey: { code: 9 } }],
    [1, { PressKey: { code: 13 } }],
    [1, { PressKey: { code: 27 } }],
  ]);

  if (type === "textarea") {
    return weighted([
      [1, keycodes],
      [4, { TypeText: { text: { Text: [1, 100] }, delayMillis } }],
    ]).generate();
  }

  switch (type) {
    case "text":
      return weighted([
        [1, keycodes],
        [3, { TypeText: { text: { Text: [1, 100] }, delayMillis } }],
      ]).generate();
    case "email":
      return weighted([
        [1, keycodes],
        [3, { TypeText: { text: "Email", delayMillis } }],
      ]).generate();
    case "number":
      return weighted([
        [1, keycodes],
        [
          3,
          {
            TypeText: {
              text: {
                Regexp: "\d{1,5}",
              },
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
  if (canGoBack.current) return ["Back"];
  return [];
});

export const forward = actions(() => {
  if (canGoForwardSameOrigin.current) return ["Forward"];
  return [];
});

export const reload = actions(() => {
  if (lastAction.current !== "Reload" && lastAction.current !== "Wait")
    return ["Reload"];
  return [];
});

export const navigation = weighted([
  [10, back],
  [1, forward],
  [1, reload],
]);
