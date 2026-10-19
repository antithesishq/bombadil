import type { Cell } from "@antithesishq/bombadil/internal";

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

/**
 * A generator of action templates. Instances of this type
 * is what you need to export from your specification module
 * in order for Bombadil to register your generators.
 */
export class ActionGenerator<A> {
  constructor(public generate: () => Tree<A>) {}

  /** Transforms every action, keeping weights intact. */
  map<B>(f: (action: A) => B): ActionGenerator<B> {
    return new ActionGenerator(() => mapTree(this.generate(), f));
  }

  /** Keeps only the actions matching the predicate. */
  filter<B extends A>(
    predicate: (action: A) => action is B,
  ): ActionGenerator<B>;
  filter(predicate: (action: A) => boolean): ActionGenerator<A>;
  filter(predicate: (action: A) => boolean): ActionGenerator<A> {
    return new ActionGenerator(() => filterTree(this.generate(), predicate));
  }

  /** Runs this generator only while the condition holds. */
  when(
    condition: boolean | Cell<boolean> | (() => boolean),
  ): ActionGenerator<A> {
    return new ActionGenerator(() => {
      const enabled =
        typeof condition === "boolean"
          ? condition
          : typeof condition === "function"
            ? condition()
            : condition.current;
      return enabled ? this.generate() : { branches: [] };
    });
  }

  /** Replaces every action with an array of actions or a generator. */
  flatMap<R extends readonly unknown[] | ActionGenerator<unknown>>(
    f: (action: A) => R,
  ): ActionGenerator<Flattened<R>> {
    return new ActionGenerator(() =>
      flatMapTree(this.generate(), (action): Tree<Flattened<R>> => {
        const result = f(action);
        if (result instanceof ActionGenerator) {
          return result.generate() as Tree<Flattened<R>>;
        }
        return branch(result.map((b) => [1, leaf(b as Flattened<R>)]));
      }),
    );
  }
}

/** The action type produced by a `flatMap` callback returning `R`. */
export type Flattened<R> =
  R extends ActionGenerator<infer B>
    ? B
    : R extends readonly (infer B)[]
      ? B
      : never;

function flatMapTree<A, B>(tree: Tree<A>, f: (value: A) => Tree<B>): Tree<B> {
  if ("value" in tree) return f(tree.value);
  return {
    branches: tree.branches.map(([weight, subtree]) => [
      weight,
      flatMapTree(subtree, f),
    ]),
  };
}

function mapTree<A, B>(tree: Tree<A>, f: (value: A) => B): Tree<B> {
  return flatMapTree(tree, (value) => leaf(f(value)));
}

function filterTree<A>(tree: Tree<A>, predicate: (value: A) => boolean) {
  return flatMapTree(tree, (value) =>
    predicate(value) ? leaf(value) : { branches: [] },
  );
}

/** The discriminant in `A`, like `"Click"` or `"Back"`. */
export type ActionDiscriminant<A> = A extends string ? A : keyof A;

/** The actions in `A` of discriminant `D`. */
export type ActionOfDiscriminant<A, D> =
  | Extract<A, D>
  | Extract<A, Record<D & string, unknown>>;

/** Checks whether the action has the given discriminant and narrows the type if so. */
export function isAction<
  A extends string | object,
  D extends ActionDiscriminant<A>,
>(action: A, discriminant: D): action is ActionOfDiscriminant<A, D> & A {
  return typeof action === "string"
    ? action === discriminant
    : discriminant in (action as object);
}

export function actions<A>(generate: () => Tree<A> | A[]): ActionGenerator<A> {
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

/**
 * Generates a random Unicode string within the given length range.
 */
export type TextGenerator = { Text: Range };

/**
 * Generates a string based on one or more charsets.
 */
export type CharSetStringGenerator = { CharSet: CharSet.Entries };

/**
 * Generates a string based on a regular expression. Uses
 * Rust [regex](https://docs.rs/regex/latest/regex/) semantics.
 */
export type RegexpStringGenerator = { Regexp: string };

/**
 * Generates a string to be used as a parameter in an action.
 */
export type StringGenerator =
  | "Email"
  | TextGenerator
  | CharSetStringGenerator
  | RegexpStringGenerator;

/**
 * Encodes characther ranges or literal strings for use in action templates,
 * where Bombadil will generate concrete strings based on the charsets.
 */
export namespace CharSet {
  export type Entry = { Range: Range } | { Literal: string };

  export namespace Entry {
    /** Checks whether the entry has the given discriminant. */
    export function is<D extends "Range" | "Literal">(
      entry: Entry,
      discriminant: D,
    ): entry is Extract<Entry, Record<D, unknown>> {
      return discriminant in entry;
    }
  }

  export type Entries = Entry[];

  export function fromRange(from: number, to: number): CharSet.Entries {
    return [{ Range: [from, to] }];
  }

  export function fromLiterals(...literals: string[]): CharSet.Entries {
    return literals.map((literal) => ({ Literal: literal }));
  }

  export function union(...sets: CharSet.Entries[]): CharSet.Entries {
    // TODO: compute overlaps?
    return sets.flat(1);
  }

  /** Checks whether the string generator is a charset. */
  export function is(
    generator: StringGenerator,
  ): generator is CharSetStringGenerator {
    return typeof generator === "object" && "CharSet" in generator;
  }
}
