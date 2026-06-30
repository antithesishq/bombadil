// Specification for `btop`, the system monitor.
//
// Run with:
//
//    bombadil terminal test --specification examples/btop.ts \
//      unshare --user --pid --fork --mount-proc bash -c "rm -rf ~/.config/btop ; btop"
//
// This runs btop in a separate process namespace, meaning it can't kill other processes
// running on your system.
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

  // Clicks anywhere in the grid.
  [
    1,
    actions(() => {
      if (!size.current) return [];
      return [
        {
          Click: {
            row: [0, size.current.rows],
            column: [0, size.current.columns],
          },
        },
      ];
    }),
  ],

  // Resize to reasonable dimensions that btop can likely render within.
  [
    1,
    actions(() => [
      {
        Resize: {
          columns: [75, 120],
          rows: [20, 48],
        },
      },
    ]),
  ],
]);
