import {
  type ActionGenerator,
  type Cell,
  type JSON,
  type Tree,
} from "@antithesishq/bombadil";
import * as bombadil from "@antithesishq/bombadil";
import { Range, StringGenerator } from "@antithesishq/bombadil/actions";

export type Point<Number = number> = {
  x: Number;
  y: Number;
};

export type Action<Number = number, String = string> =
  | "Back"
  | "Forward"
  | "Reload"
  | "Wait"
  | { Click: { fingerprint: Fingerprint; point: Point<Number> } }
  | {
      DoubleClick: {
        fingerprint: Fingerprint;
        point: Point<Number>;
        delayMillis: Range;
      };
    }
  | { TypeText: { text: String; delayMillis: Range } }
  | { PressKey: { code: number } }
  | { ScrollUp: { origin: Point; distance: Number } }
  | { ScrollDown: { origin: Point; distance: Number } }
  | { SetFileInputFiles: { selector: string; files: string[] } }
  | {
      MouseDrag: {
        from: Point;
        to: Point;
        steps: Number;
        delayMillis: Number;
      };
    }
  | { SetViewport: { width: Number; height: Number } }
  | { Custom: { name: string; state: Pick<State, "document" | "window"> } };

export type ActionTemplate = Action<Range, StringGenerator>;

export interface State {
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
      stacktrace:
        | { name: string; line: number; column: number; url: string }[]
        | null;
    }[];
  };
  console: ConsoleEntry[];
  lastAction: Action | null;
}

export type NavigationEntry = {
  id: number;
  title: string;
  url: string;
};

export type ConsoleEntry = {
  timestamp: number;
  level: "warning" | "error";
  args: JSON[];
};

// Specifically-typed wrappers over the generic factory functions in
// `@antithesishq/bombadil`.

export function extract<T extends JSON>(query: (state: State) => T): Cell<T> {
  return bombadil.extract<State, T>(query);
}

export function actions(
  generate: () => Tree<ActionTemplate> | ActionTemplate[],
): ActionGenerator<ActionTemplate> {
  return bombadil.actions<ActionTemplate>(generate);
}

export function weighted(
  value: [number, ActionTemplate | ActionGenerator<ActionTemplate>][],
): ActionGenerator<ActionTemplate> {
  return bombadil.weighted<ActionTemplate>(value);
}

// Fingerprints

export type Fingerprint = {
  testId: string | null;
  id: string | null;
  role: string | null;
  accessibleName: string | null;
  tag: string;
  href: string | null;
  nameAttr: string | null;
  placeholder: string | null;
  inputType: string | null;
  textContent: string | null;
  structuralPath: string | null;
};

export function getFingerprint(el: Element): Fingerprint {
  const tag = el.tagName.toLowerCase();

  const testId =
    el.getAttribute("data-testid") ??
    el.getAttribute("data-test-id") ??
    el.getAttribute("data-cy") ??
    el.getAttribute("data-test");

  const id = el.getAttribute("id");
  const role = el.getAttribute("role");

  const accessibleName =
    el.getAttribute("aria-label") ??
    (el.getAttribute("aria-labelledby")
      ? document
          .getElementById(el.getAttribute("aria-labelledby")!)
          ?.textContent?.trim()
      : null) ??
    el.getAttribute("title");

  const href = el.getAttribute("href");
  const nameAttr = el.getAttribute("name");
  const placeholder = el.getAttribute("placeholder");
  const inputType = el.getAttribute("type");

  const rawText = el.textContent?.trim();
  const textContent =
    rawText && rawText.length > 0 && rawText.length <= 200 ? rawText : null;

  const hasStrongIdentifier =
    testId ||
    id ||
    role ||
    accessibleName ||
    href ||
    nameAttr ||
    placeholder ||
    inputType ||
    textContent;

  const structuralPath = hasStrongIdentifier ? null : getStructuralPath(el);

  return {
    tag,
    testId,
    id,
    role,
    accessibleName,
    href,
    nameAttr,
    placeholder,
    inputType,
    textContent,
    structuralPath,
  };
}

function getStructuralPath(el: Element): string {
  const parts: string[] = [];
  let current: Element | null = el;

  while (current && current !== document.documentElement) {
    const parent: HTMLElement | null = current.parentElement;
    if (!parent) break;

    const siblings = Array.from(parent.children).filter(
      (c) => c.tagName === current!.tagName,
    );
    const index = siblings.indexOf(current as HTMLElement);
    const suffix = siblings.length > 1 ? `[${index}]` : "";
    parts.unshift(`${current.tagName.toLowerCase()}${suffix}`);
    current = parent;
  }

  return parts.join(" > ");
}

export function registerCustomAction(
  name: string,
  scriptFunction: (state: Pick<State, "document" | "window">) => void,
) {
  bombadil.registerCustomAction<Pick<State, "document" | "window">>(
    name,
    scriptFunction,
  );
}
