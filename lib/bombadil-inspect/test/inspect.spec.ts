import { actions, always, extract } from "@antithesishq/bombadil";
import { lastAction } from "@antithesishq/bombadil/defaults/actions";
import { randomRange } from "@antithesishq/bombadil/random";
export * from "@antithesishq/bombadil/defaults";

const actionEntries = extract((state) =>
  [...state.document.querySelectorAll(".actions li")].map((element) => ({
    selected: element.classList.contains("selected"),
    name: element.querySelector(".action-name")?.textContent ?? null,
    text: element.querySelector(".text")?.textContent ?? null,
    time: element.querySelector("time")?.textContent ?? null,
  })),
);

function isLoading() {
  return actionEntries.current.length === 0;
}

const timelineRect = extract((state) => {
  const element = state.document.querySelector(".timeline svg");
  if (!element) return null;
  const rect = element.getBoundingClientRect();
  return {
    x: rect.x,
    y: rect.y,
    width: rect.width,
    height: rect.height,
  };
});

export const clickTimeline = actions(() => {
  const rect = timelineRect.current;
  if (!rect || isLoading()) return [];
  const point = {
    x: randomRange(rect.x, rect.x + rect.width),
    y: randomRange(rect.y, rect.y + rect.height),
  };
  return [{ Click: { name: "timeline", point } }];
});

const cursorSpan = extract((state) => {
  const cursor = state.document.querySelector(".cursor");
  const rect = cursor?.querySelector("rect");
  if (!cursor || !rect) return null;
  const style = window.getComputedStyle(cursor);
  const transform = new WebKitCSSMatrix(style.transform);
  return {
    x1: transform.e,
    x2: transform.e + rect.width.baseVal.value,
  };
});

export const clickTimelineMovesCursorCorrectly = always(() => {
  // Make sure we have a click and a timeline.
  if (!lastAction.current) return true;
  if (typeof lastAction.current !== "object") return true;
  if (!("Click" in lastAction.current)) return true;
  if (!timelineRect.current || !cursorSpan.current) return true;
  const {
    Click: { name, point },
  } = lastAction.current;
  // And that the click was within the timeline.
  if (name !== "timeline") return true;
  // Then we should end up with the cursor interval including
  // the clicked point.
  const xRelative = point.x - timelineRect.current.x;
  return (
    xRelative >= cursorSpan.current.x1 && xRelative <= cursorSpan.current.x2
  );
});
