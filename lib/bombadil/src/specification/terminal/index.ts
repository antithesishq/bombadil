import {
  actions as actionsGeneric,
  extract as extractGeneric,
  weighted as weightedGeneric,
  type ActionGenerator,
  type Cell,
  type JSON,
  type Tree,
} from "@antithesishq/bombadil";

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

/**
 * The serialized state a specification sees on each step. The Rust
 * terminal driver builds this JSON each tick from its rendered grid +
 * scrollback + last applied action. Field shapes will be expanded as
 * the driver matures.
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

// Typed wrappers over the generic factories in `@antithesishq/bombadil`.
// See the matching block in browser/index.ts for the rationale.

export function extract<T extends JSON>(
  query: (state: State) => T,
): Cell<T> {
  return extractGeneric<State, T>(query);
}

export function actions(
  generate: () => Tree<Action> | Action[],
): ActionGenerator<Action> {
  return actionsGeneric<Action>(generate);
}

export function weighted(
  value: [number, Action | ActionGenerator<Action>][],
): ActionGenerator<Action> {
  return weightedGeneric<Action>(value);
}
