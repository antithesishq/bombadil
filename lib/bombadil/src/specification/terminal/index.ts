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
  | { ScrollUp: object }
  | { ScrollDown: object };

export interface Grid {
  size: Size;
  // Returns the cells of the row at `index`. Rows are materialized lazily, so
  // read only the rows you need and use `size` to bound your iteration. An
  // out-of-bounds `index` returns `undefined`.
  row(index: number): GridCell[];
  // Returns the rendered text of the row at `index`: occupied cells contribute
  // their contents, empty cells a space, and continuation cells nothing. This
  // is the fast path for reading a row as a string and avoids materializing a
  // cell object per column. An out-of-bounds `index` returns `undefined`.
  rowText(index: number): string;
}

export type GridCell =
  | { Occupied: { contents: string, wide: boolean, style: Style } }
  | "Empty"
  | "Continuation"

export interface Style {
  // TODO
}

export interface State {
  grid: Grid;
  scrollback: Grid;
  scrollOffset: number;
  terminated: boolean;
  lastAction: Action | null;
}

export function extract<T extends JSON>(
  query: (state: State) => T,
): Cell<T> {
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
