export interface Generator<T> {
  generate(): T;
}

// Random helpers (backed by Rust's rand crate via __bombadil_random_bytes)

declare function __bombadil_random_bytes(n: number): Uint8Array;

function random_u32(): number {
  return new DataView(__bombadil_random_bytes(4).buffer).getUint32(0);
}

/** @internal */
export function random_range(min: number, max: number): number {
  if (min >= max) {
    throw new RangeError(`min (${min}) must be less than max (${max})`);
  }
  const range = max - min;
  if (range <= 0xffffffff) {
    return min + (random_u32() % range);
  }
  // For ranges exceeding 32 bits, generate a uniform float in [0, 1) with
  // 53 bits of precision (the maximum for a JS number) and scale it.
  const view = new DataView(__bombadil_random_bytes(8).buffer);
  const uniform =
    ((view.getUint32(0) >>> 5) * 0x4000000 + (view.getUint32(4) >>> 6)) /
    0x20000000000000;
  return min + Math.floor(uniform * range);
}

function random_choice<T>(items: T[]): T {
  if (items.length === 0) {
    throw new Error("cannot choose from an empty array of items");
  }
  return items[random_u32() % items.length]!;
}

// Generators

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

  minSize(value: number): StringGenerator {
    this.size.min = value;
    return this;
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
  private range = {
    min: Number.MIN_SAFE_INTEGER,
    max: Number.MAX_SAFE_INTEGER,
  };

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
