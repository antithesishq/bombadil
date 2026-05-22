import { eventually } from "@antithesishq/bombadil";
import { actions, extract } from "@antithesishq/bombadil/terminal";

const screen = extract((state) => state.rows.join("\n"));

export const typeHelloWorld = actions(() => [
  { TypeText: { text: "hello world\n" } },
]);

export const eventuallyHelloWorld = eventually(() =>
  screen.current.includes("hello world"),
);
