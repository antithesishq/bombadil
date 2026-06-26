import { eventually } from "@antithesishq/bombadil";
import { ActionGenerator, branch, leaf } from "@antithesishq/bombadil/actions";
import { ActionTemplate, extract } from "@antithesishq/bombadil/terminal";
import { typeBasicInput } from "@antithesishq/bombadil/terminal/defaults";
export {
  exitSuccess,
  noReplacementChars,
} from "@antithesishq/bombadil/terminal/defaults";

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

export const eventuallyHelloWorldOrExit = eventually(
  () =>
    nonBlankLines.current.filter((line) => line.includes("hello world"))
      .length > 5,
).within(5, "seconds");

export const typeHelloWorld = new ActionGenerator(() =>
  branch([
    [10, typeBasicInput.generate()],
    [1, leaf({ TypeText: { Regexp: "hello world" } } as ActionTemplate)],
  ]),
);
