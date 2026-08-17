import {
  CharSets,
  typeFromSet,
} from "@antithesishq/bombadil/terminal/defaults/actions";
import { ActionTemplate, weighted } from "@antithesishq/bombadil/terminal";
import { ActionGenerator } from "../actions";
export {
  exitSuccess,
  noReplacementChars,
} from "@antithesishq/bombadil/terminal/defaults/properties";

export const typeBasicInput: ActionGenerator<ActionTemplate> = weighted([
  [10, typeFromSet(CharSets.UNICODE_SAFE)],
  [10, typeFromSet(CharSets.CONTROL_COMMON)],
]);
