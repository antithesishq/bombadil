export {
  noHttpErrorCodes,
  noUncaughtExceptions,
  noUnhandledPromiseRejections,
  noConsoleErrors,
} from "@antithesishq/bombadil/browser/defaults/properties";

import {
  scroll,
  clicks,
  inputs,
  navigation,
  waitOnce,
} from "@antithesishq/bombadil/browser/defaults/actions";
import { weighted } from "@antithesishq/bombadil/browser";

export const defaultActions = weighted([
  [100, clicks],
  [100, inputs],
  [50, scroll],
  [10, navigation],
  [1, waitOnce],
]);
