import {
  Action,
  actions,
  ActionTemplate,
  State,
} from "@antithesishq/bombadil/terminal";
import { ActionGenerator, extract } from "@antithesishq/bombadil";
import { CharSet } from "@antithesishq/bombadil/actions";

export namespace CharSets {
  export const CONTROL_COMMON = CharSet.fromLiterals(
    // "\x04", // Ctrl+D
    "\r", // Enter
    "\t", // Tab
    // NOTE: "\x1a" (Ctrl+Z) deliberately excluded until we have better process control.
  );

  // Ctrl+letter editing shortcuts (Emacs-style)
  export const CONTROL_EDITING = CharSet.fromLiterals(
    "\x01", // Ctrl+A - Goto BOL
    "\x02", // Ctrl+B - Prev char
    // "\x03", // Ctrl+C
    "\x05", // Ctrl+E - Goto EOL
    "\x06", // Ctrl+F - Next char
    "\x08", // Ctrl+H - Backspace alt
    "\x0b", // Ctrl+K - Kill to EOL
    "\x0e", // Ctrl+N - Next line
    "\x10", // Ctrl+P - Prev line
    "\x15", // Ctrl+U - Delete to BOL
    "\x17", // Ctrl+W - Delete prev word
    "\x19", // Ctrl+Y - Yank
    "\x1f", // Ctrl+_ - Undo
    "\x7f", // DEL/Backspace
  );

  export const CONTROL_ARROWS = CharSet.fromLiterals(
    "\x1b[A", // Up
    "\x1b[B", // Down
    "\x1b[C", // Right
    "\x1b[D", // Left
  );

  export const CONTROL_ARROWS_MODIFIED = CharSet.fromLiterals(
    "\x1b[1;5A", // Ctrl+Up
    "\x1b[1;5B", // Ctrl+Down
    "\x1b[1;5C", // Ctrl+Right
    "\x1b[1;5D", // Ctrl+Left
    "\x1b[1;2A", // Shift+Up
    "\x1b[1;2B", // Shift+Down
    "\x1b[1;2C", // Shift+Right
    "\x1b[1;2D", // Shift+Left
  );

  export const CONTROL_EDITING_ALT = CharSet.fromLiterals(
    "\x1bf", // Alt+f - Next word
    "\x1bb", // Alt+b - Prev word
    "\x1bd", // Alt+d - Delete next word
    "\x1b<", // Alt+< - Goto BOT
    "\x1b>", // Alt+> - Goto EOT
    // "\x1b\x7f", // Alt+Backspace - Delete prev word
  );

  export const CONTROL_NAVIGATION = CharSet.fromLiterals(
    "\x1b[H", // Home
    "\x1b[F", // End
    "\x1b[3~", // Delete
    "\x1b[5~", // Page Up
    "\x1b[6~", // Page Down
    "\x1b[2~", // Insert
    "\x1b[Z", // Shift+Tab
  );

  export const CONTROL_ALL = CharSet.union(
    CONTROL_COMMON,
    CONTROL_EDITING,
    CONTROL_EDITING_ALT,
    CONTROL_ARROWS,
    CONTROL_ARROWS_MODIFIED,
    CONTROL_NAVIGATION,
  );

  export const FUNCTION_KEYS = CharSet.fromLiterals(
    // F1-F4: SS3 sequences
    "\x1bOP", // F1
    "\x1bOQ", // F2
    "\x1bOR", // F3
    "\x1bOS", // F4
    // F5-F12: CSI sequences
    "\x1b[15~", // F5
    "\x1b[17~", // F6  (gap at 16 is intentional, historical)
    "\x1b[18~", // F7
    "\x1b[20~", // F9
    "\x1b[21~", // F10 (gap at 22)
    "\x1b[23~", // F11
    "\x1b[24~", // F12
  );

  export const ASCII_PRINTABLE = CharSet.fromRange(0x20, 0x7e);

  export const UNICODE_LATIN_EXTENDED = CharSet.fromRange(0x00a0, 0x024f);
  export const UNICODE_GREEK = CharSet.fromRange(0x0370, 0x03ff);
  export const UNICODE_CYRILLIC = CharSet.fromRange(0x0400, 0x04ff);
  export const UNICODE_CJK = CharSet.fromRange(0x4e00, 0x9fff);
  export const UNICODE_HANGUL = CharSet.fromRange(0xac00, 0xd7a3);
  export const UNICODE_EMOTICONS = CharSet.fromRange(0x1f600, 0x1f64f);
  export const UNICODE_SYMBOLS_PICTOGRAPHS_SAFE = CharSet.fromRange(
    0x1f300,
    0x1f43f,
  );

  export const UNICODE_SAFE = CharSet.union(
    ASCII_PRINTABLE,
    UNICODE_LATIN_EXTENDED,
    UNICODE_GREEK,
    UNICODE_CYRILLIC,
    UNICODE_CJK,
    UNICODE_HANGUL,
    UNICODE_EMOTICONS,
    UNICODE_SYMBOLS_PICTOGRAPHS_SAFE,
  );
}

export function typeFromSet(
  set: CharSet.Entries,
): ActionGenerator<ActionTemplate> {
  return actions(() => [
    {
      TypeText: { CharSet: set },
    },
  ]);
}

export function pasteText(text: string) {
  return {
    TypeText: {
      text: "\x1b[200~" + text + "\x1b[201~",
    },
  };
}

export const lastAction = extract<State, Action | null>(
  (state) => state.lastAction,
).named("lastAction");
