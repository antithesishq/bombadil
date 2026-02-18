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

export interface Point {
  x: number;
  y: number;
}

export type Action =
  | "Back"
  | "Forward"
  | "Reload"
  | { Click: { name: string; content?: string; point: Point } }
  | { TypeText: { text: string; delayMillis: number } }
  | { PressKey: { code: number } }
  | { ScrollUp: { origin: Point; distance: number } }
  | { ScrollDown: { origin: Point; distance: number } };

// Tree

export type Tree<T> = { value: T } | { branches: [number, Tree<T>][] };

function leaf<T>(value: T): Tree<T> {
  return { value };
}

function branch<T>(branches: [number, Tree<T>][]): Tree<T> {
  for (const [weight] of branches) {
    if (!Number.isInteger(weight) || weight < 0 || weight > 0xffff) {
      throw new RangeError(
        `invalid weight ${weight}, expected integer between 0 and 65535 inclusive`,
      );
    }
  }
  return { branches };
}

// Action generators

export class ActionGenerator implements Generator<Tree<Action>> {
  constructor(public generate: () => Tree<Action>) {}
}

export function actions(
  generate: () => Tree<Action> | Action[],
): ActionGenerator {
  return new ActionGenerator(() => {
    const result = generate();
    if (Array.isArray(result)) {
      return branch(result.map((a) => [1, leaf(a)]));
    }
    return result;
  });
}

export function weighted(
  value: [number, Action | ActionGenerator][],
): ActionGenerator {
  return new ActionGenerator(() => {
    return branch(
      value.map(([w, x]) => {
        if (x instanceof ActionGenerator) {
          return [w, x.generate()] as [number, Tree<Action>];
        }
        return [w, leaf(x)] as [number, Tree<Action>];
      }),
    );
  });
}
