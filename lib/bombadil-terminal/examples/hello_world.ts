import { actions, eventually, extract } from "@antithesishq/bombadil/terminal";

// The flattened rendered grid — every row joined by newlines.
const screen = extract((state) => state.rows.join("\n"));

// The spec's only action always types "hello world\n" into the PTY.
export const typeHelloWorld = actions(() => [
  { TypeText: { text: "hello world\n" } },
]);

// Programs like `cat` echo stdin to stdout, so after at least one
// `typeHelloWorld` action the rendered grid should contain that string.
export const eventuallyHelloWorld = eventually(() =>
  screen.current.includes("hello world"),
);
