# TypeScript Reference

## @antithesishq/bombadil

### Always {.unlisted} 

```{.typescript .no-copy}
declare class Always extends Formula {
  boundMillis: number | null;
  subformula: Formula;
  constructor(boundMillis: number | null, subformula: Formula);
  within(n: number, unit: TimeUnit): Formula;
  toString(): string;
}

```

### And {.unlisted} 

```{.typescript .no-copy}
declare class And extends Formula {
  left: Formula;
  right: Formula;
  constructor(left: Formula, right: Formula);
  toString(): string;
}

```

### Eventually {.unlisted} 

```{.typescript .no-copy}
declare class Eventually extends Formula {
  boundMillis: number | null;
  subformula: Formula;
  constructor(boundMillis: number | null, subformula: Formula);
  within(n: number, unit: TimeUnit): Formula;
  toString(): string;
}

```

### Formula {.unlisted} 

```{.typescript .no-copy}
declare class Formula {
  not(): Formula;
  and(that: IntoFormula): Formula;
  or(that: IntoFormula): Formula;
  implies(that: IntoFormula): Formula;
}

```

### Implies {.unlisted} 

```{.typescript .no-copy}
declare class Implies extends Formula {
  left: Formula;
  right: Formula;
  constructor(left: Formula, right: Formula);
  toString(): string;
}

```

### Next {.unlisted} 

```{.typescript .no-copy}
declare class Next extends Formula {
  subformula: Formula;
  constructor(subformula: Formula);
  toString(): string;
}

```

### Not {.unlisted} 

```{.typescript .no-copy}
declare class Not extends Formula {
  subformula: Formula;
  constructor(subformula: Formula);
  toString(): string;
}

```

### Or {.unlisted} 

```{.typescript .no-copy}
declare class Or extends Formula {
  left: Formula;
  right: Formula;
  constructor(left: Formula, right: Formula);
}

```

### Pure {.unlisted} 

```{.typescript .no-copy}
declare class Pure extends Formula {
  private pretty;
  value: boolean;
  constructor(pretty: string, value: boolean);
  toString(): string;
}

```

### Thunk {.unlisted} 

```{.typescript .no-copy}
declare class Thunk extends Formula {
  private pretty;
  apply: () => Formula;
  constructor(pretty: string, apply: () => Formula);
  toString(): string;
}

```

### actions {.unlisted} 

```{.typescript .no-copy}
declare function actions<A>(generate: () => Tree<A> | A[]): ActionGenerator<A>;

```

### always {.unlisted} 

```{.typescript .no-copy}
declare function always(x: IntoFormula): Always;

```

### eventually {.unlisted} 

```{.typescript .no-copy}
declare function eventually(x: IntoFormula): Eventually;

```

### extract {.unlisted} 

```{.typescript .no-copy}
declare function extract<
  S,
  T extends JSON
>(query: (state: S) => T): ExtractorCell<T, S>;

```

### next {.unlisted} 

```{.typescript .no-copy}
declare function next(x: IntoFormula): Formula;

```

### not {.unlisted} 

```{.typescript .no-copy}
declare function not(value: IntoFormula): Not;

```

### now {.unlisted} 

```{.typescript .no-copy}
declare function now(x: IntoFormula): Formula;

```

### weighted {.unlisted} 

```{.typescript .no-copy}
declare function weighted<A>(value: [number, A | ActionGenerator<A>][]): ActionGenerator<A>;

```

## @antithesishq/bombadil/actions

### ActionGenerator {.unlisted} 

```{.typescript .no-copy}
declare class ActionGenerator<A> {
  generate: () => Tree<A>;
  constructor(generate: () => Tree<A>);
}

```

### actions {.unlisted} 

```{.typescript .no-copy}
declare function actions<A>(generate: () => Tree<A> | A[]): ActionGenerator<A>;

```

### branch {.unlisted} 

```{.typescript .no-copy}
declare function branch<T>(branches: [number, Tree<T>][]): Tree<T>;

```

### fromLiterals {.unlisted} 

```{.typescript .no-copy}
function fromLiterals(...literals: string[]): CharSet.Entries;

```

### fromRange {.unlisted} 

```{.typescript .no-copy}
function fromRange(from: number, to: number): CharSet.Entries;

```

### leaf {.unlisted} 

```{.typescript .no-copy}
declare function leaf<T>(value: T): Tree<T>;

```

### union {.unlisted} 

```{.typescript .no-copy}
function union(...sets: CharSet.Entries[]): CharSet.Entries;

```

### weighted {.unlisted} 

```{.typescript .no-copy}
declare function weighted<A>(value: [number, A | ActionGenerator<A>][]): ActionGenerator<A>;

```

## @antithesishq/bombadil/browser

### State {.unlisted} 

```{.typescript .no-copy}
interface State {
  document: HTMLDocument;
  window: Window;
  navigationHistory: {
    back: NavigationEntry[];
    current: NavigationEntry;
    forward: NavigationEntry[];
  };
  errors: {
    uncaughtExceptions: {
      text: string;
      line: number;
      column: number;
      url: string | null;
      remote_object: {
        type_name: string;
        subtype: string | null;
        class_name: string | null;
        description: string | null;
        value: unknown;
      } | null;
      stacktrace: {
        name: string;
        line: number;
        column: number;
        url: string;
      }[] | null;
    }[];
  };
  console: ConsoleEntry[];
  lastAction: Action | null;
}

```

### actions {.unlisted} 

```{.typescript .no-copy}
declare function actions(generate: () => Tree<ActionTemplate> | ActionTemplate[]): ActionGenerator<ActionTemplate>;

```

### extract {.unlisted} 

```{.typescript .no-copy}
declare function extract<T extends JSON>(query: (state: State) => T): Cell<T>;

```

### getFingerprint {.unlisted} 

```{.typescript .no-copy}
declare function getFingerprint(el: Element): Fingerprint;

```

### weighted {.unlisted} 

```{.typescript .no-copy}
declare function weighted(value: [number, ActionTemplate | ActionGenerator<ActionTemplate>][]): ActionGenerator<ActionTemplate>;

```

## @antithesishq/bombadil/browser/defaults

## @antithesishq/bombadil/browser/defaults/actions

## @antithesishq/bombadil/browser/defaults/properties

## @antithesishq/bombadil/terminal

### Cursor {.unlisted} 

```{.typescript .no-copy}
interface Cursor {
  position: CursorPosition;
  visible: boolean;
  blinking: boolean;
  visualStyle: CursorVisualStyle;
  color: Color;
}

```

### CursorPosition {.unlisted} 

```{.typescript .no-copy}
interface CursorPosition {
  column: number;
  row: number;
}

```

### Grid {.unlisted} 

```{.typescript .no-copy}
interface Grid {
  size: Size;
  row(index: number): GridCell[];
  rowText(index: number): string;
}

```

### GridCell {.unlisted} 

```{.typescript .no-copy}
interface GridCell {
  /**
  * The cell's text. A single space for an empty cell, and the empty string
  * for a continuation cell (the trailing half of a wide character).
  * Concatenating `contents` across a row reconstructs `rowText`.
  */
  contents: string;
  wide: boolean;
  style: Style;
}

```

### State {.unlisted} 

```{.typescript .no-copy}
interface State {
  grid: Grid;
  scrollback: Grid;
  scrollOffset: number;
  cursor: Cursor;
  exitStatus: {
    code: number;
    signal: string | null;
  } | null;
  lastAction: Action | null;
}

```

### Style {.unlisted} 

```{.typescript .no-copy}
interface Style {
  foregroundColor: Color;
  backgroundColor: Color;
  underlineColor: Color;
  underline: Underline;
  attributes: number;
}

```

### actions {.unlisted} 

```{.typescript .no-copy}
declare function actions(generate: () => Tree<ActionTemplate> | ActionTemplate[]): ActionGenerator<ActionTemplate>;

```

### extract {.unlisted} 

```{.typescript .no-copy}
declare function extract<T extends JSON>(query: (state: State) => T): Cell<T>;

```

### has {.unlisted} 

```{.typescript .no-copy}
function has(style: Style, attribute: Attributes): boolean;

```

### weighted {.unlisted} 

```{.typescript .no-copy}
declare function weighted(value: [number, ActionTemplate | ActionGenerator<ActionTemplate>][]): ActionGenerator<ActionTemplate>;

```

## @antithesishq/bombadil/terminal/defaults

## @antithesishq/bombadil/terminal/defaults/actions

### pasteText {.unlisted} 

```{.typescript .no-copy}
declare function pasteText(text: string): {
  TypeText: {
    text: string;
  };
};

```

### typeFromSet {.unlisted} 

```{.typescript .no-copy}
declare function typeFromSet(set: CharSet.Entries): ActionGenerator<ActionTemplate>;

```

## @antithesishq/bombadil/terminal/defaults/properties

