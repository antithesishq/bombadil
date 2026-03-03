import { incr } from "./shared.ts";

export const x = 10;
export default incr(20);
export const foo = {
  bar: 30,
};
export const { x: y } = { x: 123 };
export const [z = 0] = [1];
export const [{ a: b = 1 }] = [{}];
