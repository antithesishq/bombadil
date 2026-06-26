import { always, next, now } from "@antithesishq/bombadil";
import { actions, extract, weighted } from "@antithesishq/bombadil/terminal";
import { CharSets } from "@antithesishq/bombadil/terminal/defaults/actions";
import {
  lastAction,
  typeFromSet,
} from "@antithesishq/bombadil/terminal/defaults/actions";

const statusLine = extract((state) => {
  for (let index = state.grid.size.rows - 1; index >= 0; index--) {
    const text = state.grid.rowText(index);
    if (text.trim()) {
      return { line: index, text };
    }
  }
  return null;
});

export const typeRandom = weighted([
  [40, typeFromSet(CharSets.UNICODE_SAFE)],
  [40, typeFromSet(CharSets.CONTROL_COMMON)],
  [
    1,
    actions(() => {
      const line = statusLine.current?.line;
      if (!line) return [];
      const column = statusLine.current.text.indexOf("keybindings");
      if (column < 0) return [];
      return [{ Click: { row: line, column } }];
    }),
  ],
  [
    5,
    actions(() => [
      {
        Resize: {
          columns: [80, 120],
          rows: [24, 48],
        }
      }
    ]),
  ],
]);

function justExited(): boolean {
  return (
    !!lastAction.current &&
    "TypeText" in lastAction.current &&
    lastAction.current.TypeText.includes("\x03")
  );
}

function hasIndicator(): boolean {
  return (
    !!statusLine.current &&
    statusLine.current.text.split(/\s+/).some((word) => !!word.match(/\d+:\d+/))
  );
}

export const hasLineColumnIndicator = always(
  now(() => !justExited())
    .and(next(() => !justExited()))
    .implies(next(next(() => justExited() || hasIndicator()))),
);
