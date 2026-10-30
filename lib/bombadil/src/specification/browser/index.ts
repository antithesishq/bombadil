import {
  CustomAction,
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
  | CustomAction<JSON[]>
  | "Back"
  | "Forward"
  | "Reload"
  | "Wait"
  | { Click: { fingerprint: Fingerprint; point: Point<Number> } }
  | {
      DoubleClick: {
        fingerprint: Fingerprint;
        point: Point<Number>;
        /**
         * @deprecated has no effect
         */
        delayMillis?: Range;
      };
    }
  | {
      TypeText: {
        text: String;
        /**
         * @deprecated has no effect
         */
        delayMillis?: Range;
      };
    }
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
  | { SetViewport: { width: Number; height: Number } };

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
  resources: Resources;
}

// Per-state page metrics from Chrome DevTools `Performance.getMetrics`.
// Times are floating-point seconds.
export interface Resources {
  js_heap_used: number;
  js_heap_total: number;
  dom_nodes: number;
  documents: number;
  js_event_listeners: number;
  layout_objects: number;
  timestamp: number;
  thread_time: number;
  task_duration: number;
  script_duration: number;
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

// Per-session analytics and campaign parameters (`_gl`, `utm_*`, ad click ids),
// which would make every page a new location.
const TRACKING_PARAMETERS = new Set([
  "_gl",
  "gclid",
  "dclid",
  "gbraid",
  "wbraid",
  "fbclid",
  "msclkid",
  "twclid",
  "yclid",
  "igshid",
  "mc_cid",
  "mc_eid",
  "hsCtaTracking",
]);

function isTrackingParameter(key: string): boolean {
  return (
    TRACKING_PARAMETERS.has(key) ||
    key.startsWith("utm_") ||
    key.startsWith("_ga") ||
    key.startsWith("_hs")
  );
}

const DIGITS = /^[0-9]+$/;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

// All digits or a UUID becomes `#id`, so that `/posts/42` and `/posts/43` are
// one place to the key.
function maskId(value: string): string {
  return DIGITS.test(value) || UUID.test(value) ? "#id" : value;
}

function decode(part: string): string {
  try {
    return decodeURIComponent(part.replace(/\+/g, " "));
  } catch {
    return part;
  }
}

function compare(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

// The page's location for the state key: the path and the fragment with id
// segments masked (`/posts/42/` → `/posts/#id`, `#/posts/42` → `#/posts/#id`),
// and the query, sorted, without tracking parameters and with id values
// masked. No host. It is registered here so that every browser specification
// has it. It is not exported, so it is named here.
bombadil.extract((state: State) => {
  const location = state.window.location;
  const pathname =
    "/" +
    location.pathname
      .split("/")
      .filter((segment) => segment.length > 0)
      .map(maskId)
      .join("/");
  const hash = location.hash.split("/").map(maskId).join("/");
  const query: [string, string][] = [];
  for (const pair of location.search.replace(/^\?/, "").split("&")) {
    if (!pair) continue;
    const eq = pair.indexOf("=");
    const key = decode(eq < 0 ? pair : pair.slice(0, eq));
    if (isTrackingParameter(key)) continue;
    query.push([key, eq < 0 ? "" : maskId(decode(pair.slice(eq + 1)))]);
  }
  query.sort(
    ([keyA, valueA], [keyB, valueB]) =>
      compare(keyA, keyB) || compare(valueA, valueB),
  );
  return { pathname, hash, query };
}).named("abstractLocation");

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

/**
 * Register a new custom action, returning a function used to create
 * action templates in generators. Name must be unique.
 *
 * The first two parameters to the handler are `Document` and `Window`, and then
 * any arguments passed to the function.
 */
export function registerCustomAction<Args extends JSON[]>(
  name: string,
  handler: (document: Document, window: Window, ...args: Args) => Promise<void>,
): (...args: Args) => ActionTemplate {
  return bombadil.registerCustomAction(name, (...args) =>
    handler(document, window, ...args),
  );
}

// Fingerprints

/**
 * DOM element fingerprint. String fields are either non-blank or `null`, and
 * `structuralPath` is set iff all other nullable fields are `null`.
 */
export type Fingerprint = {
  testId: string | null;
  /** The `id` attribute, or `null` when it looks generated by a framework. */
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

function nonBlank(value: string | null | undefined): string | null {
  return value && value.trim().length > 0 ? value : null;
}

// React `useId` on the client: `:r1:` (18), `«r1»` (19.0), `_r_1_` (19.1+).
// Libraries derive ids from it with a prefix or a suffix (`radix-:r1:`,
// `:r1:-label`), so the token counts wherever it stands. The server form
// (`:R1:`) is the element's position in the tree and is kept.
const ID_GENERATED_REACT =
  /:r[0-9a-z]+:|«r[0-9a-z]+»|(?:^|-)_r_[0-9a-z]+_(?:-|$)/;

// The counters MUI, radix and headlessui used before `useId`.
const ID_GENERATED_COUNTER =
  /^(?:mui|radix)-\d+(?:-|$)|^headlessui-[a-z-]+-\d+$/;

// A nanoid of the default length. Letters that do not read as words tell it
// from a written id: a capital after a lowercase letter that starts no word
// (`xQ7`), or capitals running into lowercase (`QKm`).
const ID_GENERATED_NANOID =
  /^(?=[\w-]{21}$).*(?:[a-z][A-Z](?![a-z])|[A-Z]{2}[a-z])/;

function idIsGenerated(id: string): boolean {
  return (
    ID_GENERATED_REACT.test(id) ||
    ID_GENERATED_COUNTER.test(id) ||
    ID_GENERATED_NANOID.test(id)
  );
}

export function getFingerprint(element: Element): Fingerprint {
  const attribute = (name: string) => nonBlank(element.getAttribute(name));

  const tag = element.tagName.toLowerCase();

  const testId =
    attribute("data-testid") ??
    attribute("data-test-id") ??
    attribute("data-cy") ??
    attribute("data-test");

  // A generated id is not stable across reloads, so it is left out.
  const idRaw = attribute("id");
  const id = idRaw && idIsGenerated(idRaw) ? null : idRaw;
  const role = attribute("role");

  const labelledBy = attribute("aria-labelledby");
  const accessibleName =
    attribute("aria-label") ??
    (labelledBy
      ? nonBlank(document.getElementById(labelledBy)?.textContent?.trim())
      : null) ??
    attribute("title");

  const href = attribute("href");
  const nameAttr = attribute("name");
  const placeholder = attribute("placeholder");
  const inputType = attribute("type");

  const rawText = nonBlank(element.textContent?.trim());
  const textContent = rawText && rawText.length <= 200 ? rawText : null;

  const hasIdentifier = [
    testId,
    id,
    role,
    accessibleName,
    href,
    nameAttr,
    placeholder,
    inputType,
    textContent,
  ].some((value) => value !== null);

  const structuralPath = hasIdentifier
    ? null
    : (nonBlank(getStructuralPath(element)) ?? tag);

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
