import {
  Action,
  actions,
  ActionTemplate,
  Node,
  nodes,
  Rect,
  spans,
  State,
} from "@antithesishq/bombadil/swiftui";
import { ActionGenerator, extract } from "@antithesishq/bombadil";
import { CharSet } from "@antithesishq/bombadil/actions";

export namespace CharSets {
  export const UNICODE_SAFE = CharSet.union(
    CharSet.ASCII_PRINTABLE,
    CharSet.UNICODE_LATIN_EXTENDED,
    CharSet.UNICODE_CJK,
    CharSet.UNICODE_EMOTICONS,
  );
}

// Accessibility roles that respond to taps or keyboard focus.
const INTERACTIVE_ROLES = new Set([
  "Button",
  "PopUpButton",
  "MenuButton",
  "MenuItem",
  "CheckBox",
  "Toggle",
  "Switch",
  "RadioButton",
  "Link",
  "Slider",
  "Stepper",
  "Incrementor",
  "SegmentedControl",
  "TabGroup",
  "DisclosureTriangle",
  "TextField",
  "SecureTextField",
  "TextArea",
  "SearchField",
  "ComboBox",
  "Cell",
  "Row",
]);

const EDITABLE_ROLES = new Set([
  "TextField",
  "SecureTextField",
  "TextArea",
  "SearchField",
  "ComboBox",
]);

function visible(node: Node): boolean {
  return node.frame.width > 0 && node.frame.height > 0;
}

// Frames of all enabled, visible nodes with an interactive role.
// Deliberately projected down to frames: whole nodes would drag their
// entire subtrees into every state snapshot and trace entry.
export const interactiveFrames = extract<State, Rect[]>((state) =>
  nodes(state.root)
    .filter(
      (node) =>
        node.enabled && visible(node) && INTERACTIVE_ROLES.has(node.role),
    )
    .map((node) => node.frame),
).named("interactiveFrames");

// Whether an editable text element currently has keyboard focus.
export const editableFocused = extract<State, boolean>((state) =>
  nodes(state.root).some(
    (node) => node.focused && EDITABLE_ROLES.has(node.role),
  ),
).named("editableFocused");

// The frames of all windows, for undirected taps and scrolls.
export const windowFrames = extract<State, Rect[]>((state) =>
  (state.root?.children ?? []).map((window) => window.frame),
).named("windowFrames");

// Tap a uniformly chosen point within an interactive element.
export const tapInteractive = actions(() =>
  (interactiveFrames.current ?? []).map((frame) => ({ Tap: spans(frame) })),
);

// Tap anywhere within a window.
export const tapAnywhere = actions(() =>
  (windowFrames.current ?? []).map((frame) => ({ Tap: spans(frame) })),
);

// Scroll up or down somewhere within a window.
export const scrollAnywhere = actions(() =>
  (windowFrames.current ?? []).flatMap((frame): ActionTemplate[] => {
    const scroll = { ...spans(frame), distance: [10, 200] as [number, number] };
    return [{ ScrollUp: scroll }, { ScrollDown: scroll }];
  }),
);

export function typeFromSet(
  set: CharSet.Entries,
): ActionGenerator<ActionTemplate> {
  return actions(() => [
    {
      TypeText: { CharSet: set },
    },
  ]);
}

// Type into a focused editable element; generates nothing otherwise.
export const typeWhenFocused = actions(() => {
  if (!editableFocused.current) {
    return [];
  }
  return [{ TypeText: { CharSet: CharSets.UNICODE_SAFE } }];
});

export const COMMON_KEYS = [
  "return",
  "tab",
  "escape",
  "delete",
  "space",
  "up",
  "down",
  "left",
  "right",
];

export const pressCommonKeys = actions(() =>
  COMMON_KEYS.map((key) => ({ PressKey: key })),
);

export const lastAction = extract<State, Action | null>(
  (state) => state.lastAction,
).named("lastAction");
