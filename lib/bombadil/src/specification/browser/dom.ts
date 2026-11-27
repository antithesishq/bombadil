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

export function isVisible(window: Window, element: Element): boolean {
  const style = window.getComputedStyle(element);
  return (
    style.display !== "none" &&
    style.visibility !== "hidden" &&
    parseFloat(style.opacity || "1") > 0.0
  );
}

type Point = { x: number; y: number };

type Cover = {
  // Null in another document than the top one.
  rect: DOMRect | null;
  // Where a click lands on it, unless it is inside an interactive element.
  point: Point | null;
};

/** What the hit tests of one extraction share. */
export type HitTests = {
  window: Window;
  width: number;
  height: number;
  // Where each frame's viewport starts in its parent's, null if transformed.
  frames: Map<Element, Point | null>;
  // Elements found on top of a target's centre.
  covers: Map<Element, Cover>;
};

export function hitTests(window: Window): HitTests {
  return {
    window,
    width: window.innerWidth,
    height: window.innerHeight,
    frames: new Map(),
    covers: new Map(),
  };
}

const INTERACTIVE = "a,button,input,select,textarea,label,[role]";

/**
 * A point in the top viewport where a click lands on the element: on it,
 * inside it, or on a label of it. Looks into shadow roots and same-origin
 * iframes.
 *
 * Tries the centre of the bounding box, then the centre of each of the first
 * four line fragments of a wrapped element, or else the four points halfway
 * from the centre to each corner. An element free only elsewhere is skipped.
 */
export function hitTestPoint(tests: HitTests, element: Element): Point | null {
  const rect = element.getBoundingClientRect();
  if (!(rect.width > 0 && rect.height > 0)) return null;

  let offsetX = 0;
  let offsetY = 0;
  let view: Window | null = element.ownerDocument.defaultView;
  while (view !== tests.window) {
    const frame: Element | null | undefined = view?.frameElement;
    const origin = frame && frameOrigin(tests, frame);
    if (!view || !origin) return null;
    offsetX += origin.x;
    offsetY += origin.y;
    view = view.parent;
  }

  const left = offsetX + rect.left;
  const top = offsetY + rect.top;
  const right = left + rect.width;
  const bottom = top + rect.height;
  if (right <= 0 || left >= tests.width || bottom <= 0 || top >= tests.height)
    return null;

  const centerX = left + rect.width / 2;
  const centerY = top + rect.height / 2;
  const hit = elementAt(tests, centerX, centerY);
  if (hit) {
    if (landsOn(element, hit)) return { x: centerX, y: centerY };

    // An ancestor at the centre is not on top of the element: the centre of
    // a wrapped link is between its lines.
    if (!isInside(element, hit)) {
      let cover = tests.covers.get(hit);
      if (!cover) {
        cover = {
          rect:
            hit.ownerDocument === tests.window.document
              ? hit.getBoundingClientRect()
              : null,
          point: hit.closest(INTERACTIVE) ? null : { x: centerX, y: centerY },
        };
        tests.covers.set(hit, cover);
      }
      // Under all of the cover's box, no other point is free either.
      const over = cover.rect;
      if (
        over &&
        over.left <= left &&
        over.top <= top &&
        over.right >= right &&
        over.bottom >= bottom
      )
        return null;
    }
  }

  const fragments = element.getClientRects();
  if (fragments.length > 1) {
    for (let i = 0; i < fragments.length && i < 4; i++) {
      const fragment = fragments[i]!;
      const x = offsetX + fragment.left + fragment.width / 2;
      const y = offsetY + fragment.top + fragment.height / 2;
      if (landsOn(element, elementAt(tests, x, y))) return { x, y };
    }
    return null;
  }

  for (let i = 0; i < 4; i++) {
    const x = centerX + (i & 1 ? 1 : -1) * (rect.width / 4);
    const y = centerY + (i & 2 ? 1 : -1) * (rect.height / 4);
    if (landsOn(element, elementAt(tests, x, y))) return { x, y };
  }
  return null;
}

// The innermost element at a point of the top viewport, or null if there is
// none or it is in a frame we cannot look into.
function elementAt(tests: HitTests, x: number, y: number): Element | null {
  if (!(x >= 0 && x < tests.width && y >= 0 && y < tests.height)) return null;

  let root: DocumentOrShadowRoot = tests.window.document;
  let hit: Element | null = null;
  for (;;) {
    const inner = root.elementFromPoint(x, y);
    if (!inner || inner === hit) return hit;
    hit = inner;
    if (hit.shadowRoot) {
      root = hit.shadowRoot;
    } else if (hit.localName === "iframe") {
      const document = (hit as HTMLIFrameElement).contentDocument;
      const origin = document && frameOrigin(tests, hit);
      if (!document || !origin) return null;
      x -= origin.x;
      y -= origin.y;
      root = document;
    } else {
      return hit;
    }
  }
}

function landsOn(element: Element, hit: Element | null): boolean {
  if (!hit) return false;
  if (isInside(hit, element)) return true;

  // A label does not activate its control from interactive content in it.
  const label = hit.closest("label,a,button,input,select,textarea");
  return (
    label?.localName === "label" &&
    (label as HTMLLabelElement).control === element
  );
}

// In the flat tree: a slotted node is inside its slot, and a shadow tree
// inside its host.
function isInside(node: Node | null, ancestor: Element): boolean {
  while (node) {
    if (node === ancestor) return true;
    node =
      (node as Element).assignedSlot ??
      node.parentNode ??
      (node as ShadowRoot).host ??
      null;
  }
  return false;
}

function frameOrigin(tests: HitTests, frame: Element): Point | null {
  let origin = tests.frames.get(frame);
  if (origin === undefined) {
    const rect = frame.getBoundingClientRect();
    const box = frame as HTMLElement;
    // A scaled or rotated frame has a box unlike its layout size, and the
    // offset alone does not say where a point in it is.
    if (
      Math.abs(rect.width - box.offsetWidth) > 0.5 ||
      Math.abs(rect.height - box.offsetHeight) > 0.5
    ) {
      origin = null;
    } else {
      const style = frame.ownerDocument.defaultView!.getComputedStyle(frame);
      origin = {
        x: rect.left + frame.clientLeft + parseFloat(style.paddingLeft),
        y: rect.top + frame.clientTop + parseFloat(style.paddingTop),
      };
    }
    tests.frames.set(frame, origin);
  }
  return origin;
}
