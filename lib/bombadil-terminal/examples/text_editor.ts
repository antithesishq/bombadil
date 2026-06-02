import { always, eventually, from, integers, strings } from "@antithesishq/bombadil";
import { actions, extract, type Action, weighted } from "@antithesishq/bombadil/terminal";

const KEYS = [
  // "\x03", // Ctrl+C, excluded to keep program alive
  "\x04", // Ctrl+D
  "\x1a", // Ctrl+Z
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

  // Ctrl+letter shortcuts
  "\x01",        // Ctrl+A     - Goto BOL
  "\x02",        // Ctrl+B     - Prev char (Emacs)
  "\x05",        // Ctrl+E     - Goto EOL
  "\x06",        // Ctrl+F     - Next char (Emacs)
  "\x08",        // Ctrl+H     - Backspace alt
  "\x09",        // Tab
  "\x0b",        // Ctrl+K     - Kill next line
  "\x0e",        // Ctrl+N     - Next line (Emacs)
  "\x10",        // Ctrl+P     - Prev line (Emacs)
  "\x15",        // Ctrl+U     - Delete prev line
  "\x17",        // Ctrl+W     - Delete prev word
  "\x19",        // Ctrl+Y     - Yank
  "\x1f",        // Ctrl+_     - Undo (Emacs)

  // Modifier + arrow
  "\x1b[Z",      // Shift+Tab
  "\x1b[1;5A",   // Ctrl+Up
  "\x1b[1;5B",   // Ctrl+Down
  "\x1b[1;5C",   // Ctrl+Right - Next word
  "\x1b[1;5D",   // Ctrl+Left  - Prev word
  "\x1b[1;2A",   // Shift+Up
  "\x1b[1;2B",   // Shift+Down
  "\x1b[1;2C",   // Shift+Right
  "\x1b[1;2D",   // Shift+Left

  // Meta/Alt + key  (ESC prefix)
  "\x1bf",       // Alt+f      - Next word
  "\x1bb",       // Alt+b      - Prev word
  "\x1bd",       // Alt+d      - Delete next word
  "\x1b<",       // Alt+<      - Goto BOT
  "\x1b>",       // Alt+>      - Goto EOT
  "\x1b\x7f",    // Alt+Backspace - Delete prev word

  // Navigation
  "\x1b[5~",     // Page Up
  "\x1b[6~",     // Page Down
  "\x1b[2~",     // Insert

  // Bracketed paste (exercises the paste-batching code path)
  "\x1b[200~",   // Paste start
  "\x1b[201~",   // Paste end
];

const statusLine = extract(
  (state) => {
    const lines = state.rows.map((line, index) => ({ line: index, text: line })).filter(line => !!line.text.trim());
    return lines[lines.length - 1] ?? null;
  }
);

export const typeRandom = weighted([
  [40, { TypeText: { text: strings().minSize(1).maxSize(8).generate() } }],
  [10, actions(() => {
    const text = String.fromCharCode(0x20 + integers().min(0).max(95).generate());
    return [{ TypeText: { text } }];
  })],
  [20, { TypeText: { text: from(KEYS).generate() } }],
  [1, actions(() => {
    const line = statusLine.current?.line;
    if (!line) return [];

    const column = statusLine.current.text.indexOf("keybindings");

    const click =
      `\x1b[<0;${column + 1};${line + 1}M`   // left-button press
      + `\x1b[<0;${column + 1};${line + 1}m` // release
      ;

    return [
      { TypeText: { text: click } }
    ];
  })],
  [1, actions(() => [{
    Resize: {
      size: {
        columns: integers().min(40).max(80).generate(),
        rows: integers().min(10).max(30).generate(),
      },
    }
  }])],
]);

export const hasLineColumnIndicator = always(() =>
  !!statusLine.current && statusLine.current.text.split(/\s+/).some(word => !!word.match(/\d+:\d+/))
);
