import { Point, Actions, query_all } from "../actions";

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

result = (() => {
  function clickable_point(element: Element) {
    // naive calculation of center of element
    const rect = element.getBoundingClientRect();
    if (rect.width > 0 && rect.height > 0) {
      return { x: rect.left + (rect.width / 2), y: rect.top + (rect.height / 2) };
    } else {
      return null;
    }
  }

  function is_visible(element: Element) {
    const style = window.getComputedStyle(element);
    return style.display !== "none" && style.visibility !== "hidden" && parseFloat(style.opacity || "1") > 0.0;
  }

  type Rect = Point & {
    width: number,
    height: number,
  };

  function contains(rect: Rect, point: Point) {
    return point.x >= rect.x &&
      point.x <= (rect.x + rect.width) &&
      point.y >= rect.y &&
      point.y <= (rect.y + rect.height);
  }

  const clicks: Actions = [];
  const added = new Set<Element>();

  const url_current = new URL(window.location.toString());
  for (const anchor of query_all(document.body, "a")) {
    try {
      if (!(anchor instanceof HTMLAnchorElement)) {
        continue;
      }

      let url;
      try {
        url = new URL(anchor.href);
      } catch (e) {
        console.debug(anchor.href, "could not be parsed as a URL:", e);
        continue;
      }

      if (anchor.target === "_blank") {
        continue;
      }

      if (!url.protocol.startsWith('http')) {
        console.debug(url, "is not an http(s) URL");
        continue;
      }

      if (!url.origin.endsWith(url_current.origin)) {
        console.debug(url, "is not within domain", url_current.origin);
        continue;
      }

      if (!is_visible(anchor)) {
        console.debug(anchor, "is not visible");
        continue;
      }

      const point = clickable_point(anchor);
      if (!point) {
        console.debug(anchor, "is not clickable");
        continue;
      }

      const viewport = { x: 0, y: 0, width: window.innerWidth, height: window.innerHeight };
      if (!contains(viewport, point)) {
        console.debug(anchor, "is out of viewport");
        continue;
      }

      if (added.has(anchor)) {
        continue;
      }

      console.debug('adding click action', anchor);
      clicks.push([1, 500, {
        Click: {
          name: anchor.nodeName,
          content: anchor.textContent.trim().replaceAll(/\s+/g, " "),
          point,
        }
      }]);

      added.add(anchor);
    } catch (e) {
      console.error(e);
      continue;
    }
  }
  for (const element of query_all(document.body, "button,input,textarea,label[for]")) {
    try {
      // We require visibility except for input elements, which are often hidden and overlayed with custom styling.
      if (!(element instanceof HTMLInputElement) && !is_visible(element)) {
        console.debug(element, "is not visible");
        continue;
      }

      const point = clickable_point(element);
      if (!point) {
        console.debug(element, "is not clickable");
        continue;
      }

      const viewport = { x: 0, y: 0, width: window.innerWidth, height: window.innerHeight };
      if (!contains(viewport, point)) {
        console.debug(element, "is out of viewport");
        continue;
      }


      if (element === document.activeElement && (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement) && element.value) {
        console.debug(element, "is blank and active, no need to click");
        continue;
      }

      if (added.has(element)) {
        continue;
      }

      console.debug('adding click action', element);
      clicks.push([3, 300, {
        Click: {
          name: element.nodeName,
          content: element.textContent.trim().replaceAll(/\s+/g, " "),
          point,
        }
      }]);

      added.add(element);
    } catch (e) {
      console.error(e);
      continue;
    }
  }

  const aria_selector = ARIA_ROLES_CLICKABLE.map(role => `[role=${role}]`).join(",");
  for (const element of query_all(document.body, aria_selector)) {
    try {
      if (!is_visible(element)) {
        console.debug(element, "is not visible");
        continue;
      }

      const point = clickable_point(element);
      if (!point) {
        console.debug(element, "is not clickable");
        continue;
      }

      const viewport = { x: 0, y: 0, width: window.innerWidth, height: window.innerHeight };
      if (!contains(viewport, point)) {
        console.debug(element, "is out of viewport");
        continue;
      }

      if (added.has(element)) {
        continue;
      }

      console.debug('adding click action', element);
      clicks.push([1, 500, {
        Click: {
          name: element.nodeName,
          content: element.textContent.trim().replaceAll(/\s+/g, " "),
          point,
        }
      }]);

      added.add(element);
    } catch (e) {
      console.error(e);
      continue;
    }
  }
  return clicks;
})();
