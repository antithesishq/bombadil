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

export class ActionGenerator<A> {
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

export type Range = number | [number, number];

export type StringGenerator =
  | "Email"
  | { Text: Range }
  | { CharSet: CharSet.Entries }
  | { Regexp: string }

export namespace CharSet {
  export type Entry =
    | { Range: Range }
    | { Literal: string }

  export type Entries = Entry[];

  export function fromRange(from: number, to: number): CharSet.Entries {
    return [{ Range: [from, to] }];
  }

  export function fromLiterals(...literals: string[]): CharSet.Entries {
    return literals.map((literal) => ({ Literal: literal }))
      ;
  }

  export function union(...sets: CharSet.Entries[]): CharSet.Entries {
    // TODO: compute overlaps?
    return sets.flat(1);
  }
}
