import { weighted } from "@antithesishq/bombadil/swiftui";
import {
  pressCommonKeys,
  scrollAnywhere,
  tapInteractive,
  typeWhenFocused,
} from "@antithesishq/bombadil/swiftui/defaults/actions";
export { noCrash } from "@antithesishq/bombadil/swiftui/defaults/properties";

export const defaultActions = weighted([
  [10, tapInteractive],
  [5, typeWhenFocused],
  [2, pressCommonKeys],
  [1, scrollAnywhere],
]);
