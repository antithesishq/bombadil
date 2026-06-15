import { not, always } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/terminal";

const exitStatus = extract((state) => state.exitStatus);

export const exitSuccess = always(
  not(
    () =>
      !!exitStatus.current &&
      exitStatus.current.signal == null &&
      exitStatus.current.code > 0,
  ),
);

function toHex(str: string): string {
  var result = "";
  for (var i = 0; i < str.length; i++) {
    result += str.charCodeAt(i).toString(16);
  }
  return result;
}

const replacementChars = extract((state) => {
  const result = [];
  for (let i = 0; i < state.grid.size.rows; i++) {
    for (const match of state.grid.rowText(i).matchAll(/\uFFFD/g)) {
      result.push({
        row: i,
        column: match.index,
        contents: match[0],
        hex: toHex(match[0]),
      });
    }
  }
  return result;
});

export const noReplacementChars = always(
  () => (replacementChars.current ?? []).length === 0,
);
