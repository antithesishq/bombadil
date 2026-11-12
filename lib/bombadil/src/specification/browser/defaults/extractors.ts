import { extract } from "@antithesishq/bombadil/browser";

const LENGTH = "(\\d*\\.?\\d+[a-z]*)";
const COMPARISON = "(<=|>=|<|>)";
const WIDTH_FEATURE = new RegExp(
  `\\b(min|max)-width\\s*:\\s*${LENGTH}` +
    `|${LENGTH}\\s*${COMPARISON}\\s*(?=width\\b)` +
    `|(?<=\\bwidth)\\s*${COMPARISON}\\s*${LENGTH}`,
  "g",
);

// Adds one media query per width threshold in a media condition, written so
// that it matches exactly when the viewport is past the threshold:
// `(max-width: 600px)` and `(width > 600px)` both give `(width > 600px)`.
// Reads `min-width`, `max-width` and the range syntax with a plain length;
// `calc()` and other functions are not read.
export function widthThresholds(
  condition: string,
  thresholds: Set<string>,
): void {
  for (const match of condition.matchAll(WIDTH_FEATURE)) {
    let length: string;
    let inclusive: boolean;
    if (match[1] !== undefined) {
      length = match[2]!;
      inclusive = match[1] === "min";
    } else if (match[3] !== undefined) {
      length = match[3];
      inclusive = match[4] === "<=" || match[4] === ">";
    } else {
      length = match[6]!;
      inclusive = match[5] === ">=" || match[5] === "<";
    }
    thresholds.add(
      inclusive ? `(min-width: ${length})` : `(width > ${length})`,
    );
  }
}

function collectFromRules(rules: CSSRuleList, thresholds: Set<string>): void {
  for (let index = 0; index < rules.length; index++) {
    const rule = rules[index]!;
    if (rule instanceof CSSMediaRule) {
      widthThresholds(rule.media.mediaText, thresholds);
    } else if (rule instanceof CSSImportRule) {
      widthThresholds(rule.media.mediaText, thresholds);
      if (rule.styleSheet) collectFromSheet(rule.styleSheet, thresholds);
    }
    if (rule instanceof CSSGroupingRule || rule instanceof CSSStyleRule) {
      collectFromRules(rule.cssRules, thresholds);
    }
  }
}

function collectFromSheet(sheet: CSSStyleSheet, thresholds: Set<string>): void {
  widthThresholds(sheet.media.mediaText, thresholds);
  let rules: CSSRuleList;
  try {
    rules = sheet.cssRules;
  } catch {
    // A cross-origin sheet.
    return;
  }
  collectFromRules(rules, thresholds);
}

function ruleCount(sheet: CSSStyleSheet): number {
  try {
    return sheet.cssRules.length;
  } catch {
    return -1;
  }
}

interface SheetThresholds {
  ruleCount: number;
  thresholds: Set<string>;
}

interface DocumentThresholds {
  sheets: SheetThresholds[];
  queries: MediaQueryList[];
}

// A sheet is read again only when its number of top-level rules changes, and
// the queries are rebuilt only when some sheet was read again or the list of
// sheets changed.
const thresholdsBySheet = new WeakMap<CSSStyleSheet, SheetThresholds>();
const thresholdsByDocument = new WeakMap<Document, DocumentThresholds>();

function sheetThresholds(sheet: CSSStyleSheet): SheetThresholds {
  const count = ruleCount(sheet);
  let cached = thresholdsBySheet.get(sheet);
  if (cached === undefined || cached.ruleCount !== count) {
    cached = { ruleCount: count, thresholds: new Set() };
    collectFromSheet(sheet, cached.thresholds);
    thresholdsBySheet.set(sheet, cached);
  }
  return cached;
}

function documentThresholds(
  document: Document,
  window: Window,
): MediaQueryList[] {
  const sheets = document.styleSheets;
  const cached = thresholdsByDocument.get(document);
  let stale = cached === undefined || cached.sheets.length !== sheets.length;
  for (let index = 0; index < sheets.length; index++) {
    if (sheetThresholds(sheets[index]!) !== cached?.sheets[index]) {
      stale = true;
    }
  }
  if (!stale) return cached!.queries;

  const current: SheetThresholds[] = [];
  const thresholds = new Set<string>();
  for (let index = 0; index < sheets.length; index++) {
    const sheet = thresholdsBySheet.get(sheets[index]!)!;
    current.push(sheet);
    for (const threshold of sheet.thresholds) thresholds.add(threshold);
  }
  const queries: MediaQueryList[] = [];
  for (const threshold of thresholds)
    queries.push(window.matchMedia(threshold));
  thresholdsByDocument.set(document, { sheets: current, queries });
  return queries;
}

// The number of width thresholds in the page's own media queries that the
// viewport is past, so it changes at the app's breakpoints and a spec can key
// and steer on it without hard-coding any. Thresholds come from `@media` and
// `@import` rules at any nesting depth and from the `media` of `<link>` and
// `<style>`. 0 means the viewport is narrower than every threshold, or that
// none could be read (cross-origin sheets).
export const mediaBand = extract((state) => {
  let band = 0;
  for (const query of documentThresholds(state.document, state.window)) {
    if (query.matches) band += 1;
  }
  return band;
}).setStateKeyBehavior("default");
