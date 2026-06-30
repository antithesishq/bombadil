import { ActionGenerator, CharSet, leaf } from "@antithesishq/bombadil/actions";
import { actions, extract, weighted } from "@antithesishq/bombadil/terminal";
import { CharSets } from "@antithesishq/bombadil/terminal/defaults/actions";
import { typeFromSet } from "@antithesishq/bombadil/terminal/defaults/actions";

const ui = extract((state) => {
  let working = false;
  for (let index = state.grid.size.rows - 1; index >= 0; index--) {
    // const text = state.grid.rowText(index).trim();
    const cells = state.grid.row(index);
    working =
      working ||
      cells.some(
        (cell) =>
          cell.contents === "…" && cell.style.foregroundColor !== "None",
      );
  }
  return { working };
});

export const typeRandom = new ActionGenerator(() =>
  weighted([
    // noop
    [ui.current?.working ? 1000 : 10, typeFromSet(CharSet.fromLiterals(""))],
    [40, typeFromSet(CharSets.UNICODE_SAFE)],
    [40, typeFromSet(CharSets.ASCII_PRINTABLE)],
    [40, typeFromSet(CharSets.CONTROL_ALL)],
    [5, { ScrollUp: {} }],
    [1, { ScrollDown: {} }],
    [5, typeFromSet(CharSet.fromLiterals("\r\n"))],
    [
      1,
      actions(() => [
        {
          Resize: {
            columns: [80, 120],
            rows: [24, 100],
          },
        },
      ]),
    ],
  ]).generate(),
);
