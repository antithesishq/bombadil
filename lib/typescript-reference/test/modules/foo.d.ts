// Very cool.
export declare const other: number;
/**
 * I'm a doc comment.
 */
export declare namespace Other {
  export function inner(): Internal;
  function unexported(): Internal;
  const INNER: number;
}

type Internal = number;
const internal: number;

export type Bar = number;
