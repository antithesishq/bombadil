import {
  ExtractorCell,
  Runtime,
  type Cell,
  type JSON,
} from "@antithesishq/bombadil/internal";
import {
  makeActions,
  makeWeighted,
  type ActionGenerator,
  type Tree,
} from "@antithesishq/bombadil/actions";

// Re-export the generic LTL Formula API.
export {
  Formula,
  Pure,
  Thunk,
  Not,
  And,
  Or,
  Implies,
  Next,
  Always,
  Eventually,
  now,
  next,
  always,
  eventually,
  not,
} from "@antithesishq/bombadil";
export type { Cell, JSON } from "@antithesishq/bombadil/internal";
export {
  ActionGenerator,
  type Tree,
  type Generator,
  from,
  strings,
  emails,
  integers,
  keycodes,
  randomRange,
} from "@antithesishq/bombadil/actions";

export type Size = {
  columns: number;
  rows: number;
};

/**
 * Actions a terminal driver can apply to the system under test. The
 * payload shapes mirror what the Rust `TerminalAction` enum deserializes
 * (camelCase, finite numbers). The driver validates ranges at apply
 * time.
 */
export type Action =
  | { TypeText: { text: string } }
  | { PressKey: { code: number } }
  | { Resize: { size: Size } }
  | { ScrollUp: object }
  | { ScrollDown: object };

export function actions(
  generate: () => Tree<Action> | Action[],
): ActionGenerator<Action> {
  return makeActions(generate);
}

export function weighted(
  value: [number, Action | ActionGenerator<Action>][],
): ActionGenerator<Action> {
  return makeWeighted(value);
}

/** @internal */
export const runtime = new Runtime<State>();

export function extract<T extends JSON>(query: (state: State) => T): Cell<T> {
  return new ExtractorCell<T, State>(runtime, query);
}

/**
 * The serialized state a spec sees on each step. The Rust terminal
 * driver builds this JSON each tick from its rendered grid + scrollback
 * + last applied action. Field shapes will be expanded as the driver
 * matures.
 */
export interface State {
  size: Size;
  /** Plain-text rendering of the visible viewport, row by row. */
  rows: string[];
  /** Plain-text rendering of the scrollback ring, oldest line first. */
  scrollback: string[];
  /** Number of rows the user has scrolled away from the live bottom. */
  scrollOffset: number;
  /** Whether the underlying process has exited. */
  finished: boolean;
  lastAction: Action | null;
}
