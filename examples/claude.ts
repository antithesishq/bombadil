/// This is a spec for Claude Code, the TUI coding agent from Anthropic.
import { ActionGenerator, CharSet } from "@antithesishq/bombadil/actions";
import { actions, extract, weighted } from "@antithesishq/bombadil/terminal";
import { CharSets } from "@antithesishq/bombadil/terminal/defaults/actions";
import { typeFromSet } from "@antithesishq/bombadil/terminal/defaults/actions";

const ui = extract((state) => {
  let working = false;
  for (let index = state.grid.size.rows - 1; index >= 0; index--) {
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
    // Noop, most likely when Claude says it's working, to give it a chance to
    // finish. We do still allow for input, which usually means queued messages.
    [ui.current?.working ? 1000 : 10, typeFromSet(CharSet.fromLiterals(""))],

    // Otherwise, we hammer on with various inputs and control sequences.
    [40, typeFromSet(CharSets.UNICODE_SAFE)],
    [40, typeFromSet(CharSets.ASCII_PRINTABLE)],
    [40, typeFromSet(CharSets.CONTROL_ALL)],

    // Submit message.
    [5, typeFromSet(CharSet.fromLiterals("\r\n"))],

    // We also scroll and resize the terminal.
    [5, { ScrollUp: {} }],
    [1, { ScrollDown: {} }],
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
