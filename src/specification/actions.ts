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

// Generators

export class ActionGenerator implements Generator<Action[]> {
  constructor(public generate: () => Action[]) {}
}

export function actions(generate: () => Action[]): ActionGenerator {
  return new ActionGenerator(generate);
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
  generate() {
    const len = random_range(1, 16);
    return Array.from({ length: len }, () => random_choice([...ALPHANUMERIC])).join("");
  }
}

export function strings(): Generator<string> {
  return new StringGenerator();
}

class EmailGenerator implements Generator<string> {
  generate() {
    const user = Array.from({ length: random_range(3, 10) }, () => random_choice([...ALPHANUMERIC])).join("");
    const domain = Array.from({ length: random_range(3, 8) }, () => random_choice([...ALPHANUMERIC])).join("");
    return `${user}@${domain}.com`;
  }
}

export function emails(): Generator<string> {
  return new EmailGenerator();
}

class IntegerGenerator implements Generator<string> {
  generate() {
    return random_range(0, 10000).toString();
  }
}

export function integers(): Generator<string> {
  return new IntegerGenerator();
}

class KeycodeGenerator implements Generator<number> {
  static CODES = [8, 9, 13, 27];
  generate() {
    return random_choice(KeycodeGenerator.CODES)!;
  }
}

export function keycodes(): Generator<number> {
  return new KeycodeGenerator();
}
