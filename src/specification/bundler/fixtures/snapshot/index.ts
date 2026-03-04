import z, { x, foo as bar } from "./other.ts";
import { incr } from "./subdirectory/shared.ts";

export const y = incr(x + z + bar.bar);
export * from "./other.ts";
