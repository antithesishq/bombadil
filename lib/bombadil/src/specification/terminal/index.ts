import {
  type ActionGenerator,
  type Cell,
  type JSON,
  type Tree,
} from "@antithesishq/bombadil";
import * as bombadil from "@antithesishq/bombadil";
import { Range, StringGenerator } from "@antithesishq/bombadil/actions";

export type Size<Number = number> = {
  columns: Number;
  rows: Number;
};

export type Action<Number = number, String = string> =
  | { TypeText: String }
  | { Resize: Size<Number> }
  | { Click: { row: Range, column: Range } }
  | { ScrollUp: {} }
  | { ScrollDown: {} };

export type ActionTemplate = Action<Range, StringGenerator>;

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

export interface Cursor {
  // Zero-indexed cursor position in the terminal's active screen.
  position: CursorPosition;
  visible: boolean;
  blinking: boolean;
  visualStyle: CursorVisualStyle;
  color: Color;
}

export interface CursorPosition {
  column: number;
  row: number;
}

export type CursorVisualStyle =
  | "Bar"
  | "Block"
  | "Underline"
  | "BlockHollow"
  | "Unknown";

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
  cursor: Cursor;
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
  generate: () => Tree<ActionTemplate> | ActionTemplate[],
): ActionGenerator<ActionTemplate> {
  return bombadil.actions<ActionTemplate>(generate);
}

export function weighted(
  value: [number, ActionTemplate | ActionGenerator<ActionTemplate>][],
): ActionGenerator<ActionTemplate> {
  return bombadil.weighted<ActionTemplate>(value);
}
