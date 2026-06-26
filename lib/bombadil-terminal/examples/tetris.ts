import { always } from "@antithesishq/bombadil";
import { branch, CharSet } from "@antithesishq/bombadil/actions";
import { actions, extract } from "@antithesishq/bombadil/terminal";
import {
  typeFromSet,
  CharSets,
} from "@antithesishq/bombadil/terminal/defaults/actions";

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

export const notBlank = always(() => nonBlankLines.current.length > 0);

const isPlaying = extract((state) => {
  let score = false;
  let gameOver = false;

  for (let index = 0; index < state.grid.size.rows; index++) {
    const text = state.grid.rowText(index);
    if (text.includes("Score")) {
      score = true;
    }
    if (text.includes("GAME OVER")) {
      gameOver = true;
    }
  }

  return score && !gameOver;
});

export const tetrisActions = actions(() => {
  if (isPlaying.current) {
    return branch([
      [1, typeFromSet(CharSet.fromLiterals(" ", "p")).generate()],
      [1, typeFromSet(CharSets.CONTROL_ARROWS).generate()],
    ]);
  }

  return branch([
    [1, typeFromSet(CharSets.UNICODE_SAFE).generate()],
    [1, typeFromSet(CharSets.CONTROL_COMMON).generate()],
    [1, typeFromSet(CharSets.CONTROL_ARROWS).generate()],
  ]);
});
