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

      const click =
        `\x1b[<0;${column + 1};${line + 1}M` + // left-button press
        `\x1b[<0;${column + 1};${line + 1}m`; // release

      return [{ TypeText: { text: click } }];
    }),
  ],
  // TODO: restore when ghostty doesn't have the overflow on resize
  // [
  //   1,
  //   actions(() => [
  //     {
  //       Resize: {
  //         size: {
  //           columns: integers().min(40).max(80).generate(),
  //           rows: integers().min(10).max(30).generate(),
  //         },
  //       },
  //     },
  //   ]),
  // ],
]);

function justExited(): boolean {
  return (
    !!lastAction.current &&
    "TypeText" in lastAction.current &&
    lastAction.current.TypeText.text.includes("\x03")
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
