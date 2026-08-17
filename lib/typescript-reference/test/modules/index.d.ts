export { other } from "@example/test/foo";
/**
 * This is the docs.
 */
export default function add(a: number, b: number): number;
// It's good.
export declare function multiply(a: number, b: number): number;
// The interface.
export interface PiResult {
  // The result.
  pi: number;
}
// Gets the number.
export declare function getPi(): PiResult;
// I'm a magic number.
export declare const MAGIC_NUMBER: number;
