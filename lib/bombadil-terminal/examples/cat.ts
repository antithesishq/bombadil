import { eventually } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/terminal";
export * from "@antithesishq/bombadil/terminal/defaults";

const nonBlankLines = extract((state) => {
  const lines = [];
  for (let index = 0; index < state.grid.size.rows; index++) {
    const text = state.grid.rowText(index).trim();
    if (text) {
      lines.push(text);
    }
  }
  return lines;
});

export const eventuallyHelloWorld = eventually(() =>
  nonBlankLines.current.every((line) => line === "hello world"),
);
