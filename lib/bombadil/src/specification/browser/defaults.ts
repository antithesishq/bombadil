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
import { ActionGenerator } from "@antithesishq/bombadil";
import { ActionTemplate, weighted } from "@antithesishq/bombadil/browser";

export const defaultActions: ActionGenerator<ActionTemplate> = weighted([
  [100, clicks],
  [100, inputs],
  [50, scroll],
  [10, navigation],
  [1, waitOnce],
]);
