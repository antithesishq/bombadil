import { eventually, from, integers, strings } from "@antithesishq/bombadil";
import { actions, extract, type Action } from "@antithesishq/bombadil/terminal";

const KEYS = [
  "\x03", // Ctrl+C
  "\x04", // Ctrl+D
  "\x1b", // Escape
  "\r", // Enter
  "\x7f", // Backspace
  "\x1b[A", // Arrow up
  "\x1b[B", // Arrow down
  "\x1b[C", // Arrow right
  "\x1b[D", // Arrow left
  "\x1b[H", // Home
  "\x1b[F", // End
  "\x1b[3~", // Delete
];

const text = from<() => string>([
  () => strings().minSize(1).maxSize(8).generate(),
  () => integers().min(0).max(10_000).generate().toString(),
  () => from(KEYS).generate(),
]);

export const typeRandom = actions((): Action[] => [
  { TypeText: { text: text.generate()() } },
]);

export const resize = actions((): Action[] => [
  {
    Resize: {
      size: {
        columns: integers().min(1).max(100).generate(),
        rows: integers().min(1).max(100).generate(),
      },
    },
  },
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
