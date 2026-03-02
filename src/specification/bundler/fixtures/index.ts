import z, { x, foo as bar } from "./other.ts";
import { incr } from "./shared.ts";

export { x } from "./other.ts";
export const y = incr(x + z + bar.bar);
