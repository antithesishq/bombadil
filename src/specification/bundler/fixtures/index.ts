import z, { x, foo as bar } from "./other.ts";

export const y = x + z + bar.bar;
