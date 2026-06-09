// Like querySelectorAll, but searches recursively into shadow roots and iframes.
export function queryAll(root: Element, selector: string): Element[] {
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
    } else if (element instanceof HTMLSlotElement) {
      // Follow assigned nodes instead of the slot's own children
      for (const assigned of element.assignedElements({ flatten: true })) {
        queue.push(assigned);
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

export function clickablePoint(
  element: Element,
): { x: number; y: number } | null {
  const rect = element.getBoundingClientRect();
  if (rect.width > 0 && rect.height > 0) {
    return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
  }
  return null;
}

export function isVisible(window: Window, element: Element): boolean {
  const style = window.getComputedStyle(element);
  return (
    style.display !== "none" &&
    style.visibility !== "hidden" &&
    parseFloat(style.opacity || "1") > 0.0
  );
}

export function inViewport(
  window: Window,
  point: { x: number; y: number },
): boolean {
  return (
    point.x >= 0 &&
    point.x <= window.innerWidth &&
    point.y >= 0 &&
    point.y <= window.innerHeight
  );
}
