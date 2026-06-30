import { always, Cell, next } from "@antithesishq/bombadil";
import { extract, weighted } from "@antithesishq/bombadil/terminal";
import { CharSets } from "@antithesishq/bombadil/terminal/defaults/actions";
import {
  lastAction,
  typeFromSet,
} from "@antithesishq/bombadil/terminal/defaults/actions";

// const statusLinesText: Cell<string> = extract((state) => {
//   const { rows } = state.grid.size;
//   if (rows < 2) return "";
//   return state.grid.rowText(rows - 2) + " " + state.grid.rowText(rows - 1);
// });

// export const hasStandardBindings = always(
//   next(() => {
//     const text = statusLinesText.current ?? "";
//     return (
//       justExited() ||
//       (text.includes("Help") && text.includes("Exit") && text.includes("Read"))
//     );
//   }),
// );
//
function justExited(): boolean {
  return (
    !!lastAction.current &&
    "TypeText" in lastAction.current &&
    (lastAction.current.TypeText.includes("\x03") ||
      lastAction.current.TypeText.includes("\x04"))
  );
}

export const typeRandom = weighted([
  [40, typeFromSet(CharSets.UNICODE_SAFE)],
  [40, typeFromSet(CharSets.CONTROL_COMMON)],
  [1, { Resize: { rows: [10, 100], columns: [40, 120] } }],
]);
