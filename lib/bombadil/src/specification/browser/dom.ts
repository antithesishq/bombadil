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

/** A wheel event at `origin` scrolls a container by up to `distance`. */
export type WheelScroll = { origin: Point; distance: number };

/** One axis of a scroll container. A direction is null when no wheel moves it. */
export type AxisScroll = {
  // The frame element for a frame's viewport.
  element: Element;
  horizontal: boolean;
  size: number;
  snaps: boolean;
  backward: WheelScroll | null;
  forward: WheelScroll | null;
};

export type WheelScrolls = {
  /** Where a wheel event scrolls the top viewport. */
  viewport: { up: Point | null; down: Point | null };
  inner: AxisScroll[];
};

type Scroller = {
  element: Element;
  // Set for a frame's viewport.
  document: Document | null;
  snaps: boolean;
  // Lengths are in top viewport pixels.
  width: number;
  height: number;
  xMin: number;
  x: number;
  xMax: number;
  yMin: number;
  y: number;
  yMax: number;
  // Never passes a scroll on to its container.
  containsX: boolean;
  containsY: boolean;
};

const LEFT = 1;
const RIGHT = 2;
const UP = 4;
const DOWN = 8;
const ALL = LEFT | RIGHT | UP | DOWN;

// Less hidden content than a line of text brings nothing new into view.
const OVERFLOW_MIN = 16;

function scrollable(scroller: Scroller): number {
  return (
    (scroller.x - scroller.xMin >= 1 ? LEFT : 0) |
    (scroller.xMax - scroller.x >= 1 ? RIGHT : 0) |
    (scroller.y - scroller.yMin >= 1 ? UP : 0) |
    (scroller.yMax - scroller.y >= 1 ? DOWN : 0)
  );
}

/**
 * The scroll containers a wheel event can scroll, including the viewports of
 * same-origin frames, and where to send it for each direction.
 */
export function wheelScrolls(tests: HitTests): WheelScrolls {
  const window = tests.window;
  const scrollers: Scroller[] = [];
  const elements = new Map<Element, Scroller>();

  const stack: Element[] = [window.document.documentElement];
  while (stack.length > 0) {
    const element = stack.pop()!;

    let children: ParentNode | null = element;
    if (element.shadowRoot) {
      children = element.shadowRoot;
    } else if (element.localName === "slot") {
      children = null;
      for (const assigned of (element as HTMLSlotElement).assignedElements({
        flatten: true,
      })) {
        stack.push(assigned);
      }
    } else if (element.localName === "iframe") {
      children = null;
      const inner = (element as HTMLIFrameElement).contentDocument;
      if (inner?.documentElement) {
        stack.push(inner.documentElement);
        const viewport = frameViewport(window, element, inner);
        if (viewport) scrollers.push(viewport);
      }
    }
    if (children) {
      for (
        let child = children.lastElementChild;
        child;
        child = child.previousElementSibling
      ) {
        stack.push(child);
      }
    }

    // The viewport scrolls for the root, and for the body when the root's
    // overflow is visible.
    const document = element.ownerDocument;
    if (element === document.documentElement) continue;

    // Style before layout: reading it is cheaper, and rules out most elements.
    const style = window.getComputedStyle(element);
    const overflow = style.overflow;
    if (!overflow.includes("auto") && !overflow.includes("scroll")) continue;
    const overflowX = style.overflowX;
    const overflowY = style.overflowY;
    const scrollsX = overflowX === "auto" || overflowX === "scroll";
    const scrollsY = overflowY === "auto" || overflowY === "scroll";
    if (style.writingMode !== "horizontal-tb") continue;
    if (element === document.body) {
      if (element === document.scrollingElement) continue;
      const root = window.getComputedStyle(document.documentElement);
      if (root.overflowX === "visible" && root.overflowY === "visible")
        continue;
    }

    const width = element.clientWidth;
    const height = element.clientHeight;
    let rangeX = scrollsX ? element.scrollWidth - width : 0;
    let rangeY = scrollsY ? element.scrollHeight - height : 0;
    if (rangeX < OVERFLOW_MIN) rangeX = 0;
    if (rangeY < OVERFLOW_MIN) rangeY = 0;
    if (rangeX === 0 && rangeY === 0) continue;

    // Content that starts at the far end has negative scroll positions.
    const flex = style.display.endsWith("flex");
    const column = flex && style.flexDirection.startsWith("column");
    const mainReversed = flex && style.flexDirection.endsWith("-reverse");
    const crossReversed = flex && style.flexWrap === "wrap-reverse";
    const rtl = style.direction === "rtl";
    const x = rangeX > 0 ? element.scrollLeft : 0;
    const y = rangeY > 0 ? element.scrollTop : 0;
    const reversedX =
      x < 0 || (x === 0 && rtl !== (column ? crossReversed : mainReversed));
    const reversedY =
      y < 0 || (y === 0 && (column ? mainReversed : crossReversed));

    // Scroll positions are in the element's own pixels.
    const zoom =
      (element as Element & { currentCSSZoom?: number }).currentCSSZoom ?? 1;
    const scroller: Scroller = {
      element,
      document: null,
      snaps: style.scrollSnapType !== "none",
      width: width * zoom,
      height: height * zoom,
      xMin: reversedX ? -rangeX * zoom : 0,
      x: x * zoom,
      xMax: reversedX ? 0 : rangeX * zoom,
      yMin: reversedY ? -rangeY * zoom : 0,
      y: y * zoom,
      yMax: reversedY ? 0 : rangeY * zoom,
      containsX: style.overscrollBehaviorX !== "auto",
      containsY: style.overscrollBehaviorY !== "auto",
    };
    scrollers.push(scroller);
    elements.set(element, scroller);
  }

  const center = { x: tests.width / 2, y: tests.height / 2 };
  const result: WheelScrolls = {
    viewport: { up: center, down: center },
    inner: [],
  };
  if (scrollers.length === 0) return result;

  const points: (Point | null)[] = [null, null, null, null];
  wheelPoints(
    tests,
    elements,
    window.document,
    UP | DOWN,
    0,
    0,
    tests.width,
    tests.height,
    points,
  );
  result.viewport.up = points[2] ?? null;
  result.viewport.down = points[3] ?? null;

  for (const scroller of scrollers) {
    const wanted = scrollable(scroller);
    if (wanted === 0) continue;

    let left = 0;
    let top = 0;
    let frame: Element | null = scroller.element;
    if (!scroller.document) {
      const rect = frame.getBoundingClientRect();
      left = rect.left + (rect.width - scroller.width) / 2;
      top = rect.top + (rect.height - scroller.height) / 2;
      frame = frame.ownerDocument.defaultView?.frameElement ?? null;
    }
    let transformed = false;
    while (frame && !transformed) {
      const offset = frameOrigin(tests, frame);
      if (offset) {
        left += offset.x;
        top += offset.y;
        frame = frame.ownerDocument.defaultView?.frameElement ?? null;
      } else {
        transformed = true;
      }
    }
    if (transformed) continue;

    wheelPoints(
      tests,
      elements,
      scroller.document ?? scroller.element,
      wanted,
      Math.max(left, 0),
      Math.max(top, 0),
      Math.min(left + scroller.width, tests.width),
      Math.min(top + scroller.height, tests.height),
      points,
    );
    const leftward = points[0] ?? null;
    const rightward = points[1] ?? null;
    const upward = points[2] ?? null;
    const downward = points[3] ?? null;
    if (leftward || rightward) {
      result.inner.push({
        element: scroller.element,
        horizontal: true,
        size: scroller.width,
        snaps: scroller.snaps,
        backward: leftward && {
          origin: leftward,
          distance: scroller.x - scroller.xMin,
        },
        forward: rightward && {
          origin: rightward,
          distance: scroller.xMax - scroller.x,
        },
      });
    }
    if (upward || downward) {
      result.inner.push({
        element: scroller.element,
        horizontal: false,
        size: scroller.height,
        snaps: scroller.snaps,
        backward: upward && {
          origin: upward,
          distance: scroller.y - scroller.yMin,
        },
        forward: downward && {
          origin: downward,
          distance: scroller.yMax - scroller.y,
        },
      });
    }
  }
  return result;
}

function frameViewport(
  window: Window,
  frame: Element,
  document: Document,
): Scroller | null {
  const root = document.scrollingElement;
  if (!root || frame.getAttribute("scrolling") === "no") return null;

  // The viewport takes the root's overflow, or else the body's.
  let style = window.getComputedStyle(document.documentElement);
  if (
    style.overflowX === "visible" &&
    style.overflowY === "visible" &&
    document.body
  ) {
    style = window.getComputedStyle(document.body);
  }
  if (style.writingMode !== "horizontal-tb") return null;
  const scrollsX = style.overflowX !== "hidden" && style.overflowX !== "clip";
  const scrollsY = style.overflowY !== "hidden" && style.overflowY !== "clip";

  const width = root.clientWidth;
  const height = root.clientHeight;
  let rangeX = scrollsX ? root.scrollWidth - width : 0;
  let rangeY = scrollsY ? root.scrollHeight - height : 0;
  if (rangeX < OVERFLOW_MIN) rangeX = 0;
  if (rangeY < OVERFLOW_MIN) rangeY = 0;
  if (rangeX === 0 && rangeY === 0) return null;

  const x = rangeX > 0 ? root.scrollLeft : 0;
  const reversedX = x < 0 || (x === 0 && style.direction === "rtl");
  return {
    element: frame,
    document,
    snaps:
      window.getComputedStyle(document.documentElement).scrollSnapType !==
      "none",
    width,
    height,
    xMin: reversedX ? -rangeX : 0,
    x,
    xMax: reversedX ? 0 : rangeX,
    yMin: 0,
    y: rangeY > 0 ? root.scrollTop : 0,
    yMax: rangeY,
    containsX: false,
    containsY: false,
  };
}

// Fills `points`, indexed left, right, up, down, with a point of the box
// from which a wheel event in that direction scrolls the target: the centre,
// or else a point halfway to a corner.
function wheelPoints(
  tests: HitTests,
  elements: Map<Element, Scroller>,
  target: Element | Document,
  wanted: number,
  left: number,
  top: number,
  right: number,
  bottom: number,
  points: (Point | null)[],
) {
  points.fill(null);
  if (right <= left || bottom <= top) return;
  for (let i = 0; i < 5 && wanted !== 0; i++) {
    const signX = i === 0 ? 0 : i & 1 ? 1 : -1;
    const signY = i === 0 ? 0 : i & 2 ? 1 : -1;
    const point = {
      x: (left + right) / 2 + (signX * (right - left)) / 4,
      y: (top + bottom) / 2 + (signY * (bottom - top)) / 4,
    };
    const reached = wanted & ~wheelBlocked(tests, elements, target, point);
    for (let direction = 0; direction < 4; direction++) {
      if (reached & (1 << direction)) points[direction] = point;
    }
    wanted &= ~reached;
  }
}

// The directions in which a wheel event at the point does not get to the
// target. It goes up the containing blocks, not the ancestors, to the first
// scroller on its axis.
function wheelBlocked(
  tests: HitTests,
  elements: Map<Element, Scroller>,
  target: Element | Document,
  point: Point,
): number {
  let blocked = 0;
  // 1 or 2 while looking for the containing block of an absolutely or fixed
  // positioned box.
  let positioned = 0;
  let node: Node | null = elementAt(tests, point.x, point.y);
  while (node) {
    if (node.nodeType === Node.DOCUMENT_NODE) {
      return node === target ? blocked : ALL;
    }
    if (node.nodeType === Node.ELEMENT_NODE) {
      const element = node as Element;
      const style = tests.window.getComputedStyle(element);
      const position = style.position;
      if (
        positioned === 0 ||
        (positioned === 1 && position !== "static") ||
        containsPositioned(style)
      ) {
        if (element === target) return blocked;
        const inner = elements.get(element);
        if (inner) {
          // Even at its end: it may have a fraction of a pixel left, and
          // then takes the event.
          if (inner.containsX || inner.xMax > inner.xMin)
            blocked |= LEFT | RIGHT;
          if (inner.containsY || inner.yMax > inner.yMin) blocked |= UP | DOWN;
          if (blocked === ALL) return ALL;
        }
        positioned = position === "absolute" ? 1 : position === "fixed" ? 2 : 0;
      }
    }
    node =
      (node as Element).assignedSlot ??
      node.parentNode ??
      (node as ShadowRoot).host ??
      null;
  }
  return ALL;
}

// Whether a box is the containing block of everything positioned in it.
function containsPositioned(style: CSSStyleDeclaration): boolean {
  return (
    style.transform !== "none" ||
    style.perspective !== "none" ||
    style.filter !== "none" ||
    (style.backdropFilter ?? "none") !== "none" ||
    (style.containerType ?? "normal") !== "normal" ||
    /paint|layout|strict|content/.test(style.contain) ||
    /transform|perspective|filter/.test(style.willChange)
  );
}
