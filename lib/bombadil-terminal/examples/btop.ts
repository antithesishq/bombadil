import { actions, extract, weighted } from "@antithesishq/bombadil/terminal";
import { integers } from "@antithesishq/bombadil/random";
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

      let line = integers().min(0).max(size.current.rows).generate();
      let column = integers().min(0).max(size.current.columns).generate();

      const click =
        `\x1b[<0;${column + 1};${line + 1}M` + // left-button press
        `\x1b[<0;${column + 1};${line + 1}m`; // release

      return [{ TypeText: { text: click } }];
    }),
  ],

  // TODO: restore once ghostty doesn't have the scroll overflow bug
  // [1, actions(() => [{
  //   Resize: {
  //     size: {
  //       columns: integers().min(80).max(120).generate(),
  //       rows: integers().min(24).max(48).generate(),
  //     },
  //   }
  // }])],
]);
