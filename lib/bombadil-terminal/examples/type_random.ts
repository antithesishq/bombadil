import {
  eventually,
  from,
  integers,
  strings,
} from "@antithesishq/bombadil";
import { actions, extract, type Action } from "@antithesishq/bombadil/terminal";

// One thunk per text-generation strategy. `from` picks one thunk each
// step; invoking it calls `.generate()` on the underlying primitive.
// Wrapping the primitive generators in thunks is the workaround for the
// fact that `StringGenerator`, `IntegerGenerator`, and `From<string>`
// don't share a common `Generator<string>` view in the current API.
const text = from<() => string>([
  () => strings().minSize(1).maxSize(8).generate(),
  () => integers().min(0).max(10_000).generate().toString(),
  () => from(["yes", "no", "maybe", "ok\n"]).generate(),
]);

export const typeRandom = actions((): Action[] => [
  { TypeText: { text: text.generate()() } },
]);

const nonBlankRows = extract(
  (state) => state.rows.filter((row) => row.trim().length > 0).length,
);

// With an echoing program like `cat`, the first applied action should
// render at least one non-blank row. Bounded so the run can't loop
// forever if the SUT isn't actually echoing back.
export const eventuallyEchoes = eventually(
  () => nonBlankRows.current >= 1,
).within(5, "seconds");
