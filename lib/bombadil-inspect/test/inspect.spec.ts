import { actions, always, eventually, extract } from "@antithesishq/bombadil";
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
    left: transform.e,
    right: transform.e + rect.width.baseVal.value,
  };
});

const chartSpan = extract((state) => {
  const background = state.document.querySelector(".line-chart .background");
  if (!background) return null;
  const rect = background.getBoundingClientRect();
  return {
    left: rect.left,
    right: rect.right,
  };
});

export const eventuallyShowsActions = always(
  eventually(() => actionEntries.current.length > 0).within(2, "seconds"),
);

export const clickTimelineMovesCursorCorrectly = always(() => {
  // Make sure we have a click and a timeline.
  if (!lastAction.current) return true;
  if (typeof lastAction.current !== "object") return true;
  if (!("Click" in lastAction.current)) return true;
  if (!timelineRect.current || !chartSpan.current || !cursorSpan.current)
    return true;
  const {
    Click: { name, point },
  } = lastAction.current;
  // And that the click was within the timeline.
  if (name !== "timeline") return true;

  // If the click is left of the timeline, we pick the first transition.
  if (point.x <= chartSpan.current.left) {
    return (
      Math.floor(cursorSpan.current.left) == Math.floor(chartSpan.current.left)
    );
  }

  // If the click is right of the timeline, we pick the last transition.
  if (point.x >= chartSpan.current.right) {
    return (
      Math.ceil(cursorSpan.current.right) == Math.ceil(chartSpan.current.right)
    );
  }

  // Otherwise we should end up with the cursor interval including
  // the clicked point.
  const xRelative = point.x - timelineRect.current.x;
  return (
    xRelative >= Math.floor(cursorSpan.current.left) &&
    xRelative <= Math.ceil(cursorSpan.current.right)
  );
});
