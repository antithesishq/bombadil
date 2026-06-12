import {
  type ActionGenerator,
  type Cell,
  type JSON,
  type Tree,
} from "@antithesishq/bombadil";
import * as bombadil from "@antithesishq/bombadil";

export type Size = {
  columns: number;
  rows: number;
};

export type Action =
  | { TypeText: { text: string } }
  | { PressKey: { code: number } }
  | { Resize: { size: Size } }
  | { ScrollUp: {} }
  | { ScrollDown: {} };

export interface Grid {
  size: Size;
  // @returns the cells of the row at `index`.
  row(index: number): GridCell[];
  // @returns the rendered text of the row at `index`. Use {@link row}
  // if you need styling information of the individual grid cells.
  rowText(index: number): string;
}

export interface GridCell {
  // The cell's text. A single space for an empty cell, and the empty string
  // for a continuation cell (the trailing half of a wide character).
  // Concatenating `contents` across a row reconstructs {@link Grid.rowText}.
  contents: string;
  wide: boolean;
  style: Style;
}

export type Color =
  | "None"
  | { Palette: number }
  | { RGB: { r: number; g: number; b: number } };

export type Underline =
  | "None"
  | "Single"
  | "Double"
  | "Curly"
  | "Dotted"
  | "Dashed";

// Bit flags packed into {@link Style.attributes}. Use {@link Attributes.has}
// to test membership rather than the bitwise operators directly.
export enum Attributes {
  Bold = 0b00000001,
  Italic = 0b00000010,
  Blink = 0b00000100,
  Inverse = 0b00001000,
  Strikethrough = 0b00010000,
  Dim = 0b00100000,
  Invisible = 0b01000000,
  Overline = 0b10000000,
}

export namespace Attributes {
  // @returns whether `style` has the given `attribute` set.
  export function has(style: Style, attribute: Attributes): boolean {
    return (style.attributes & attribute) !== 0;
  }
}

export interface Style {
  foregroundColor: Color;
  backgroundColor: Color;
  underlineColor: Color;
  underline: Underline;
  // Bit mask of the {@link Attributes} flags. Query with {@link Attributes.has}.
  attributes: number;
}

export interface State {
  grid: Grid;
  scrollback: Grid;
  scrollOffset: number;
  exitStatus: {
    code: number;
    signal: string | null;
  } | null;
  lastAction: Action | null;
}

export function extract<T extends JSON>(query: (state: State) => T): Cell<T> {
  return bombadil.extract<State, T>(query);
}

export function actions(
  generate: () => Tree<Action> | Action[],
): ActionGenerator<Action> {
  return bombadil.actions<Action>(generate);
}

export function weighted(
  value: [number, Action | ActionGenerator<Action>][],
): ActionGenerator<Action> {
  return bombadil.weighted<Action>(value);
}
