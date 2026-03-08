import z, { x, foo as bar } from "./other.ts";
import { incr } from "./subdirectory/shared.ts";
import addFn, { multiply, MAGIC_NUMBER } from "test-lib";
// @ts-ignore
import file from "./file.txt";
import data from "./data.json";
import nspell from "nspell";

export const y = incr(x + z + bar.bar);
export const computed = multiply(addFn(1, 2), MAGIC_NUMBER);
export * from "./other.ts";
export const text = file;
export const jsonx = data.x;
export const spell = nspell;
