
import { eventually } from "@antithesishq/bombadil";
import { actions, extract } from "@antithesishq/bombadil/terminal";

const screen = extract((state) => {
  const lines = [];
  for (let index = 0; index < state.grid.size.rows; index++) {
    lines.push(state.grid.rowText(index));
  }
  return lines.join("\n");
});

export const eventuallyReady = eventually(
  () => screen.current.includes("ready"),
);

export const noop = actions(() => [{ TypeText: { text: "" } }]);
