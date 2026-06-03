
import { eventually } from "@antithesishq/bombadil";
import { actions, extract, GridCell } from "@antithesishq/bombadil/terminal";

function cellToString(cell: GridCell) {
  switch (cell) {
    case "Empty":
      return " ";
    case "Continuation":
      return "";
    default:
      return cell.Occupied.contents;
  }
}

const screen = extract((state) =>
  state.grid.rows.flatMap(row => row.map(cellToString)).join("\n"));

export const eventuallyReady = eventually(
  () => screen.current.includes("ready"),
);

export const noop = actions(() => [{ TypeText: { text: "" } }]);
