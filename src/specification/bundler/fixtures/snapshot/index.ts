import z, { x, foo as bar } from "./other.ts";
import { incr } from "./subdirectory/shared.ts";
import addFn, { multiply, MAGIC_NUMBER } from "test-lib";

export const y = incr(x + z + bar.bar);
export const computed = multiply(addFn(1, 2), MAGIC_NUMBER);
export * from "./other.ts";
