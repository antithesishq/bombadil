export {
  noHttpErrorCodes,
  noUncaughtExceptions,
  noUnhandledPromiseRejections,
  noConsoleErrors,
} from "@antithesishq/bombadil/defaults/properties";

import {
  scroll,
  clicks,
  inputs,
  navigation,
  waitOnce,
} from "@antithesishq/bombadil/defaults/actions";
import { weighted } from "@antithesishq/bombadil/actions";

export const defaultActions = weighted([
  [10, clicks],
  [10, inputs],
  [5, scroll],
  [1, navigation],
  [1, waitOnce],
]);
