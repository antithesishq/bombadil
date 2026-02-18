export interface Point {
  x: number;
  y: number;
}

export type Action =
  | "Back"
  | "Forward"
  | "Reload"
  | { Click: { name: string; content?: string; point: Point } }
  | { TypeText: { text: string; delay_millis: number } }
  | { PressKey: { code: number } }
  | { ScrollUp: { origin: Point; distance: number } }
  | { ScrollDown: { origin: Point; distance: number } };

export interface Generator<T> {
  generate(): T;
}

// Random helpers (backed by Rust's rand crate via __bombadil_random_bytes)

declare function __bombadil_random_bytes(n: number): Uint8Array;

function random_u32(): number {
  return new DataView(__bombadil_random_bytes(4).buffer).getUint32(0);
}

function random_range(min: number, max: number): number {
  return min + (random_u32() % (max - min));
}

function random_choice<T>(items: T[]): T {
  return items[random_u32() % items.length]!;
}

// Tree

export type Tree<T> = { value: T } | { branches: [number, Tree<T>][] };

function leaf<T>(value: T): Tree<T> {
  return { value };
}

function branch<T>(branches: [number, Tree<T>][]): Tree<T> {
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

export class From<T> implements Generator<T> {
  constructor(private elements: T[]) {}

  generate() {
    return random_choice(this.elements);
  }
}

export function from<T>(elements: T[]): From<T> {
  if (elements.length === 0) {
    throw new Error("`from` needs at least one element");
  }
  return new From(elements);
}

const ALPHANUMERIC = "abcdefghijklmnopqrstuvwxyz0123456789";

class StringGenerator implements Generator<string> {
  private size = { min: 0, max: 16 };
  generate() {
    const len = random_range(this.size.min, this.size.max);
    return Array.from({ length: len }, () =>
      random_choice([...ALPHANUMERIC]),
    ).join("");
  }

  maxSize(value: number): StringGenerator {
    this.size.max = value;
    return this;
  }
}

export function strings(): StringGenerator {
  return new StringGenerator();
}

class EmailGenerator implements Generator<string> {
  generate() {
    const user = Array.from({ length: random_range(3, 10) }, () =>
      random_choice([...ALPHANUMERIC]),
    ).join("");
    const domain = Array.from({ length: random_range(3, 8) }, () =>
      random_choice([...ALPHANUMERIC]),
    ).join("");
    return `${user}@${domain}.com`;
  }
}

export function emails(): Generator<string> {
  return new EmailGenerator();
}

class IntegerGenerator implements Generator<number> {
  private range = { min: Number.MIN_VALUE, max: Number.MAX_VALUE };

  generate() {
    return random_range(this.range.min, this.range.max);
  }

  min(value: number): IntegerGenerator {
    this.range.min = value;
    return this;
  }

  max(value: number): IntegerGenerator {
    this.range.max = value;
    return this;
  }
}

export function integers(): IntegerGenerator {
  return new IntegerGenerator();
}

export function keycodes(): Generator<number> {
  return from([8, 9, 13, 27]);
}
