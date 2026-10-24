import { always, Formula } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";

// Per-session analytics and campaign parameters (`_gl`, `utm_*`, ad click ids),
// which would make every page a new location.
function isTrackingParameter(key: string): boolean {
  return (
    key.startsWith("utm_") ||
    key.startsWith("_ga") ||
    key.startsWith("_hs") ||
    [
      "_gl", "gclid", "dclid", "gbraid", "wbraid", "fbclid", "msclkid", "twclid",
      "yclid", "igshid", "mc_cid", "mc_eid", "hsCtaTracking",
    ].includes(key)
  );
}

// All digits or a UUID. Masked as `#id`, so that `/posts/42` and `/posts/43`
// are one place to the key.
function isId(value: string): boolean {
  return /^[0-9]+$/.test(value) || /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/.test(value);
}

function decode(part: string): string {
  try {
    return decodeURIComponent(part.replace(/\+/g, " "));
  } catch {
    return part;
  }
}

// The page's location for the state key: the path with ids masked (`/posts/42/`
// → `/posts/#id`), the fragment, and the query without tracking parameters and
// with id values masked. No host.
export const abstractLocation = extract((state) => {
  const { pathname: rawPath, search, hash } = state.window.location;
  const pathname = "/" + rawPath.split("/").filter((s) => s.length > 0).map((s) => (isId(s) ? "#id" : s)).join("/");
  const query: [string, string][] = [];
  for (const pair of search.replace(/^\?/, "").split("&")) {
    if (!pair) continue;
    const eq = pair.indexOf("=");
    const key = decode(eq < 0 ? pair : pair.slice(0, eq));
    const value = eq < 0 ? "" : decode(pair.slice(eq + 1));
    if (isTrackingParameter(key)) continue;
    query.push([key, isId(value) ? "#id" : value]);
  }
  query.sort();
  return { pathname, hash, query };
});

const responseStatus = extract((state) => {
  const first = state.window.performance.getEntriesByType("navigation")[0];
  return first && first instanceof PerformanceNavigationTiming
    ? first.responseStatus
    : null;
});

export const noHttpErrorCodes: Formula = always(
  () => (responseStatus.current ?? 0) < 400,
);

function formatException(e: {
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
}): string {
  let result = e.text;
  if (e.remote_object?.description) {
    const firstLine = e.remote_object.description.split("\n")[0];
    result = `${e.text} ${firstLine}`;
  } else if (
    e.remote_object?.value !== null &&
    e.remote_object?.value !== undefined
  ) {
    const value = String(e.remote_object.value);
    if (value && value !== e.text) {
      result = `${e.text} ${value}`;
    }
  }
  if (e.stacktrace) {
    for (const frame of e.stacktrace) {
      result += "\n    at ";
      if (frame.name) result += frame.name + " ";
      result += `(${frame.url}:${frame.line}:${frame.column})`;
    }
  } else if (e.url) {
    result += `\n    at ${e.url}:${e.line}:${e.column}`;
  }
  return result;
}

const uncaughtExceptions = extract((state) =>
  state.errors.uncaughtExceptions.map(formatException),
);

export const noUncaughtExceptions: Formula = always(() =>
  uncaughtExceptions.current.every(
    (e) => !e.startsWith("Uncaught") || e.startsWith("Uncaught (in promise)"),
  ),
);

export const noUnhandledPromiseRejections: Formula = always(() =>
  uncaughtExceptions.current.every(
    (e) => !e.startsWith("Uncaught (in promise)"),
  ),
);

// No timestamp or stack, so that the same error gives the same snapshot and can
// take part in the state key.
const consoleErrors = extract((state) =>
  state.console
    .filter((e) => e.level === "error")
    .map((e) => ({
      level: e.level,
      message: e.args
        .map((a) => (typeof a === "string" ? a : JSON.stringify(a)))
        .join(" ")
        .split("\n")[0],
    })),
);

export const noConsoleErrors: Formula = always(
  () => consoleErrors.current?.length === 0,
);
