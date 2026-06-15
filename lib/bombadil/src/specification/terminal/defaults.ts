import {
  CharSets,
  typeFromSet,
} from "@antithesishq/bombadil/terminal/defaults/actions";
import { weighted } from "@antithesishq/bombadil/terminal";
export { exitSuccess } from "@antithesishq/bombadil/terminal/defaults/properties";

export const defaultActions = weighted([
  [10, typeFromSet(CharSets.UNICODE_SAFE)],
  [10, typeFromSet(CharSets.CONTROL_COMMON)],
]);
