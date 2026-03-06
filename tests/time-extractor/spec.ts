import { actions, extract, always, time } from "@antithesishq/bombadil";

// Extract counter value and current time
const counter = extract((state) => parseInt(state.document.querySelector("#counter")?.textContent || "0"));
const timeSnapshot = extract((state) => time.current);

export const _actions = actions(() => [
  { Click: "#btn" },
]);

// Property: time should be non-decreasing
export const time_is_non_decreasing = always(() => {
  return timeSnapshot.previous === undefined || timeSnapshot.current >= timeSnapshot.previous;
});
