import { type Generator } from "@antithesishq/bombadil/random";

export type { Generator } from "@antithesishq/bombadil/random";
export {
  from,
  strings,
  emails,
  integers,
  keycodes,
  randomRange,
} from "@antithesishq/bombadil/random";

// Tree

export type Tree<T> = { value: T } | { branches: [number, Tree<T>][] };

export function leaf<T>(value: T): Tree<T> {
  return { value };
}

export function branch<T>(branches: [number, Tree<T>][]): Tree<T> {
  for (const [weight] of branches) {
    if (!Number.isInteger(weight) || weight < 0 || weight > 0xffff) {
      throw new RangeError(
        `invalid weight ${weight}, expected integer between 0 and 65535 inclusive`,
      );
    }
  }
  return { branches };
}

export class ActionGenerator<A> implements Generator<Tree<A>> {
  constructor(public generate: () => Tree<A>) { }
}

export function actions<A>(
  generate: () => Tree<A> | A[],
): ActionGenerator<A> {
  return new ActionGenerator(() => {
    const result = generate();
    if (Array.isArray(result)) {
      return branch(result.map((a) => [1, leaf(a)]));
    }
    return result;
  });
}

export function weighted<A>(
  value: [number, A | ActionGenerator<A>][],
): ActionGenerator<A> {
  return new ActionGenerator(() => {
    return branch(
      value.map(([w, x]) => {
        if (x instanceof ActionGenerator) {
          return [w, x.generate()] as [number, Tree<A>];
        }
        return [w, leaf(x)] as [number, Tree<A>];
      }),
    );
  });
}
