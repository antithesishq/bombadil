import { always, Cell, next } from "@antithesishq/bombadil";
import { extract, weighted } from "@antithesishq/bombadil/terminal";
import { CharSets } from "@antithesishq/bombadil/terminal/defaults/actions";
import {
  lastAction,
  typeFromSet,
} from "@antithesishq/bombadil/terminal/defaults/actions";

const statusLinesWords: Cell<string[]> = extract((state) => {
  if (state.grid.size.rows < 2) {
    return [];
  }
  const line1 = state.grid.rowText(state.grid.size.rows - 2);
  const line2 = state.grid.rowText(state.grid.size.rows - 1);
  return [line1, line2]
    .flatMap((line) => line.split(/\s+/))
    .map((word) => word.trim())
    .filter(Boolean);
  // .filter((word) => !word.match(/^(:?M-\w|\^.)$/));
});

function justExited(): boolean {
  return (
    !!lastAction.current &&
    "TypeText" in lastAction.current &&
    (lastAction.current.TypeText.includes("\x03") ||
      lastAction.current.TypeText.includes("\x04"))
  );
}

// export const hasStandardBindings = always(
//   now(() => !justExited())
//     .and(next(() => !justExited()))
//     .implies(
//       next(
//         next(() => {
//           const words = new Set(statusLinesWords.current ?? []);
//           return (
//             justExited() ||
//             ["Help", "Exit", "Read"].every((word) => words.has(word))
//           );
//         }),
//       ),
//     ),
// );

export const hasStandardBindings = always(
  next(() => {
    const words = new Set(statusLinesWords.current ?? []);
    return (
      justExited() || ["Help", "Exit", "Read"].every((word) => words.has(word))
    );
  }),
);

export const typeRandom = weighted([
  [40, typeFromSet(CharSets.UNICODE_SAFE)],
  [40, typeFromSet(CharSets.CONTROL_COMMON)],
]);
