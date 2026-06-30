import { actions, extract, weighted } from "@antithesishq/bombadil/terminal";
import {
  typeFromSet,
  CharSets,
} from "@antithesishq/bombadil/terminal/defaults/actions";
export * from "@antithesishq/bombadil/terminal/defaults/properties";

const size = extract((state) => {
  return state.grid.size;
});

export const typeRandom = weighted([
  [40, typeFromSet(CharSets.UNICODE_SAFE)],
  [40, typeFromSet(CharSets.CONTROL_ALL)],

  // Clicks
  [
    1,
    actions(() => {
      if (!size.current) return [];

      // const click =
      //   `\x1b[<0;${column + 1};${line + 1}M` + // left-button press
      //   `\x1b[<0;${column + 1};${line + 1}m`; // release

      return [{ Click: { row: [0, size.current.rows], column: [0, size.current.columns] } }];
    }),
  ],

  [1, actions(() => [{
    Resize: {
      columns: [80, 120],
      rows: [24, 48],
    }
  }])],
]);
