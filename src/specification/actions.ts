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

export interface Generator<T> {
  generate(): T;
}

export class ActionGenerator implements Generator<Action[]> {
  constructor(public generate: () => Action[]) {}
}

export function actions(generate: () => Action[]): ActionGenerator {
  return new ActionGenerator(generate);
}

export class From<T> implements Generator<T> {
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

class StringGenerator implements Generator<string> {
  generate() {
    // TODO: actual random generation
    return "hello";
  }
}

export function strings(): Generator<string> {
  return new StringGenerator();
}

class EmailGenerator implements Generator<string> {
  generate() {
    // TODO: actual random generation
    return "test@example.com";
  }
}

export function emails(): Generator<string> {
  return new EmailGenerator();
}

class IntegerGenerator implements Generator<string> {
  generate() {
    // TODO: actual random generation
    return (42).toString();
  }
}

export function integers(): Generator<string> {
  return new IntegerGenerator();
}

class KeycodeGenerator implements Generator<number> {
  generate() {
    // TODO: actual random generation
    return 13;
  }
}

export function keycodes(): Generator<number> {
  return new KeycodeGenerator();
}
