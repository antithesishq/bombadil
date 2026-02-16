import { Duration } from "@antithesishq/bombadil/internal";

export interface Point {
  x: number;
  y: number;
}

export type Action =
  | "Back"
  | "Reload"
  | { Click: { name: string; content?: string; point: Point } }
  | { TypeText: { text: string; delay: Duration } }
  | { PressKey: { code: number } }
  | { ScrollUp: { origin: Point; distance: number } }
  | { ScrollDown: { origin: Point; distance: number } };

export class ActionGenerator {
  // @ts-ignore
  // (generate isn't used here, but in the boa runtime)
  constructor(private generate: () => Action[]) {}
}

export function actions(generate: () => Action[]): ActionGenerator {
  return new ActionGenerator(generate);
}

export class From<T> {
  constructor(private elements: T[]) {}

  generate() {
    // TODO: actual random generation
    return this.elements[0]!;
  }
}

export function from<T>(elements: T[]): From<T> {
  if (elements.length === 0) {
    throw new Error("`from` needs at least one element");
  }
  return new From(elements);
}
