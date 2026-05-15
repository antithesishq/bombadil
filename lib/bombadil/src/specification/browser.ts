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

// Re-export the generic LTL Formula API so a spec only needs to import
// from `@antithesishq/bombadil/browser`.
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

export type Point = {
  x: number;
  y: number;
};

export type Action =
  | "Back"
  | "Forward"
  | "Reload"
  | "Wait"
  | { Click: { name: string; content?: string; point: Point } }
  | {
      DoubleClick: {
        name: string;
        content?: string;
        point: Point;
        delayMillis: number;
      };
    }
  | { TypeText: { text: string; delayMillis: number } }
  | { PressKey: { code: number } }
  | { ScrollUp: { origin: Point; distance: number } }
  | { ScrollDown: { origin: Point; distance: number } }
  | { SetFileInputFiles: { selector: string; files: string[] } };

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

export interface State {
  document: HTMLDocument;
  window: Window;
  navigationHistory: {
    back: NavigationEntry[];
    current: NavigationEntry;
    forward: NavigationEntry[];
  };
  errors: {
    uncaughtExceptions: {
      text: string;
      line: number;
      column: number;
      url: string | null;
      remote_object: {
        type_name: string;
        subtype: string | null;
        class_name: string | null;
        description: string | null;
        value: unknown;
      } | null;
      stacktrace:
        | { name: string; line: number; column: number; url: string }[]
        | null;
    }[];
  };
  console: ConsoleEntry[];
  lastAction: Action | null;
}

export type NavigationEntry = {
  id: number;
  title: string;
  url: string;
};

export type ConsoleEntry = {
  timestamp: number;
  level: "warning" | "error";
  args: JSON[];
};
