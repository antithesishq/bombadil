export { c } from "./c.ts";

import { b } from "./b.ts";
import "./sideEffect.ts";

log("a start");
export const a = 10;
log("a=" + a);
log("b=" + b);

const localX = 42;
const localY = 43;
export { localX as x, localY as y };

log("a end");
