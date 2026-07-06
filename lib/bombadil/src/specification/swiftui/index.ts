import {
  type ActionGenerator,
  type Cell,
  type JSON,
  type Tree,
} from "@antithesishq/bombadil";
import * as bombadil from "@antithesishq/bombadil";
import { Range, StringGenerator } from "@antithesishq/bombadil/actions";

export type Rect = {
  x: number;
  y: number;
  width: number;
  height: number;
};

// A node in the accessibility tree reported by the in-app agent. The
// root node represents the application; its children are windows.
export interface Node {
  // Accessibility role, e.g. "Button", "TextField", "StaticText".
  role: string;
  // The accessibilityIdentifier set by the app, if any.
  identifier?: string;
  // Accessibility label (usually the visible text).
  label?: string;
  // Accessibility value, rendered as a string.
  value?: string;
  // Frame in screen coordinates (points, origin top-left).
  frame: Rect;
  enabled: boolean;
  selected: boolean;
  focused: boolean;
  children: Node[];
}

export type Scroll<Number = number> = {
  x: Number;
  y: Number;
  distance: Number;
};

export type Action<Number = number, Text = string> =
  | { Tap: { x: Number; y: Number } }
  | { TypeText: Text }
  | { PressKey: string }
  | { ScrollUp: Scroll<Number> }
  | { ScrollDown: Scroll<Number> };

export type ActionTemplate = Action<Range, StringGenerator | string>;

export interface State {
  // The accessibility tree, or null once the app has exited.
  root: Node | null;
  exitStatus: {
    code: number;
    signal: string | null;
  } | null;
  lastAction: Action | null;
}

// @returns `node` and all of its descendants, depth-first.
export function nodes(node: Node | null | undefined): Node[] {
  if (!node) {
    return [];
  }
  const result: Node[] = [];
  const stack: Node[] = [node];
  while (stack.length > 0) {
    const current = stack.pop()!;
    result.push(current);
    for (let i = current.children.length - 1; i >= 0; i--) {
      stack.push(current.children[i]);
    }
  }
  return result;
}

// @returns the ranges spanned by the rect on each axis, for use in
// tap and scroll templates.
export function spans(frame: Rect): {
  x: [number, number];
  y: [number, number];
} {
  return {
    x: [frame.x, frame.x + frame.width],
    y: [frame.y, frame.y + frame.height],
  };
}

// @returns an action template that taps a uniformly chosen point
// within the node's frame.
export function tap(node: Node): ActionTemplate {
  return { Tap: spans(node.frame) };
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
