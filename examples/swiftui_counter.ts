// Specification for the CounterExample app in swift/BombadilAgent:
// the counter must never go below zero. Run with:
//
//   bombadil swiftui test --specification examples/swiftui_counter.ts \
//       --exit-on-violation -- <path to CounterExample>
import { always } from "@antithesishq/bombadil";
import { actions, extract, nodes, tap } from "@antithesishq/bombadil/swiftui";

const count = extract((state) => {
  const node = nodes(state.root).find((n) => n.identifier === "count");
  return node && node.value != null ? parseInt(node.value) : null;
});

const buttons = extract((state) =>
  nodes(state.root).filter((n) => n.role === "Button" && n.enabled),
);

export const tapButtons = actions(() => (buttons.current ?? []).map(tap));

export const countNonNegative = always(
  () => count.current === null || count.current >= 0,
);
