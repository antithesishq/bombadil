import { eventually, not } from "@antithesishq/bombadil";
import { actions, extract } from "@antithesishq/bombadil/terminal";

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

export const typeHelloWorld = actions(() => [
  { TypeText: { text: "hello world\n" } },
]);

export const eventuallyHelloWorld = eventually(() =>
  nonBlankLines.current.every((line) => line === "hello world"),
);

const exitCode = extract((state) => state.exitCode);

export const exitSuccess = not(
  eventually(() => !!exitCode.current && exitCode.current > 0),
);
