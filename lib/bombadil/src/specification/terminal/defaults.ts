import {
  ASCII_PRINTABLE,
  CONTROL_ALL,
  pasteText,
  typeFromSet,
} from "@antithesishq/bombadil/terminal/defaults/actions";
import { actions } from "@antithesishq/bombadil/terminal";
import { strings } from "@antithesishq/bombadil/random";
export { exitSuccess } from "@antithesishq/bombadil/terminal/defaults/properties";

export const typeAscii = typeFromSet(ASCII_PRINTABLE);

export const typeControl = typeFromSet(CONTROL_ALL);

export const pasteAny = actions(() => [
  pasteText(strings().minSize(1).maxSize(128).generate()),
]);
