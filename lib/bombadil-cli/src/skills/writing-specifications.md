# Writing Bombadil specifications

Translate the application's contract into general properties, not fixed test
scripts. Read existing specifications and the installed TypeScript definitions
before writing new ones. Use the project's `@antithesishq/bombadil` package.

Ground every property in evidence: documentation, visible UI text and labels,
existing tests, and invariants the code itself asserts. An ordinary
expectation, such as a saved item appearing in its list, is evidence too; name
it in a comment. Do not assert behavior that nothing documents or implies, and
do not restate the implementation.

## Build the specification module

A specification is a TypeScript or JavaScript ES module. Its top level must
export only properties and action generators. Give properties meaningful export
names; Bombadil uses those names in error reports. Keep helper values unexported.
You can import local modules and packages, but the runtime is limited:
do not assume Node.js APIs are available.

Start with the relevant driver defaults, then add domain-specific checks:

```typescript
// Browser specification
export * from "@antithesishq/bombadil/browser/defaults";
```

```typescript
// Terminal specification
export * from "@antithesishq/bombadil/terminal/defaults";
```

Without a custom specification, Bombadil uses these defaults. A custom module
uses what it exports; retain relevant checks and actions explicitly.
Browser defaults cover common errors and semantic HTML interactions.
Terminal defaults check successful exit and rendering problems, and provide
basic input. Extend them with properties specific to the application.

## Extract observable state and express properties

Import temporal operators from `@antithesishq/bombadil`. Import `extract` from
the relevant driver module. An extractor runs for every captured state and
returns JSON-serializable data. It produces a `Cell`, not the extracted value;
read `.current` inside a property thunk so the condition is reevaluated.

Browser example adapted from the manual's Extractors section:

```typescript
import { always } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";
export * from "@antithesishq/bombadil/browser/defaults";

const title = extract((state) => state.document.title || "");
export const hasTitle = always(() => title.current !== "");
```

For a terminal specification, import from `@antithesishq/bombadil/terminal` and
extract screen data such as `state.grid.rowText(0).trim()` instead of DOM data.
Use this property only if a nonempty title is actually part of the contract.
Keep extraction observational; do not perform actions in an extractor.

Choose the temporal meaning before writing the condition:

- `always(x)`: this state and every future state must satisfy `x`.
- `next(x)`: the next captured state must satisfy `x`.
- `eventually(x)`: this state or a future state must satisfy `x`.
- `now(() => condition)`: a condition in the current state, useful with
  `.and`, `.or`, `.implies`, and `.not`.

Use the installed API for time bounds such as `.within(5, "seconds")`, and
give each `eventually` the deadline the contract implies. An implication can
pass because its precondition never occurs. Check that actions can reach the
trigger and that the property detects a realistic failure.

## Pin values in both directions

A property that checks only that no error appears, or that an element exists,
fails only when the UI breaks visibly. Give each feature at least one property
that pins what the state means: a counter changes by exactly one per
increment, a total equals the sum of its lines, a submitted value appears where
it was sent. The manual's Examples section has state-machine and contextful
guarantee patterns for this.

Check both directions of input handling. Accept: valid input produces the
documented result. Reject: invalid or hostile input, such as empty, overlong,
or malformed values, is refused visibly and leaves state unchanged.
Export one contract per property; Bombadil reports violations by export name.

## Generate valid actions

Use the driver's `actions(() => [...])` to return action templates based on
extracted state. Return only actions possible in the current state; return `[]`
when none are valid. Bombadil selects an action and parameter values from
the weighted generator tree. Use `weighted` to adjust relative probabilities.

Prefer adapting an existing generator before writing one from scratch:

- `map` replaces an action.
- `filter` removes actions that do not satisfy a condition.
- `flatMap` replaces an action with alternatives, or drops it with `[]`.
- `when` disables a generator when its condition is false.

Transformations preserve weights. When restricting a default generator, export
the transformed generator, not the unrestricted original. The browser defaults
module exports the unrestricted `defaultActions`, so `export *` from it
defeats any restriction of those actions. In that case export the default
properties by name instead. Restrict actions only to keep runs safe or inside
the application, not to avoid a violation.

Browser custom actions use `registerCustomAction(name, handler)`. The handler
receives `Document`, `Window`, and custom arguments. Return calls to the
registered action from a guarded action generator. This can handle workflows
such as authentication or consent. Terminal custom actions are not supported;
use terminal action templates instead.

## Validate and exercise

Check imports and types against the project's installed package and use its
existing typecheck command. Run a short bounded `test` to check setup, then
prefer a bounded fuzzing campaign to find bugs:

```sh
bombadil browser test --headless --time-limit 30s --output-path browser-spec-check.bin http://localhost:3000 spec.ts
bombadil terminal test --specification tui.spec.ts --time-limit 30s --output-path terminal-spec-check.bin -- your-tui
bombadil skill show finding-bugs
```

Check in the trace that the states each property watches are reached. Do not
weaken, unexport, or delete a property to make a violation disappear. Use
`bombadil skill show investigating-failures` to distinguish application
defects from specification and setup errors.

## Before you finish

1. **Adversarial review by a subagent.** Launch a separate subagent with the
   application's contract (documentation, UI text, routes, existing tests)
   but not your specification; a reviewer holding your properties inherits
   your blind spots. Ask it to assume the application is wrong and to return
   the properties the contract implies, the inputs and action sequences most
   likely to break each, and the actions needed to reach them. Write what it
   returns as properties and actions, or state why you dropped each one.
2. **Checklist.** Run `bombadil skill show reviewing-specifications` and check
   every exported property and generator against it.
3. **Exercise.** Run a bounded campaign as described above and report what it
   found, which properties it reached, and which it did not.

## Sources and translation notes

Source revision: `69a61bc473f7ee01ceda024216ebc6921b7ea753`.
Paths refer to the Bombadil repository at that revision:

- `docs/manual/src/02-getting-started.md`: TypeScript support.
- `docs/manual/src/03-specification-language.md`: Structure; Importing modules and files; Default properties and action generators; Properties; Extractors; Formulas; Action generators; Transforming action generators; Custom actions; Examples.

The `.within` syntax is checked against `lib/bombadil/src/specification/index.ts`.
Driver imports are checked against `lib/bombadil/src/specification/browser/index.ts`
and `lib/bombadil/src/specification/terminal/index.ts`.
The `defaultActions` export is checked against
`lib/bombadil/src/specification/browser/defaults.ts`.
Nonvacuous checks, observational extraction, and the validation sequence are
agent workflow rules. Example properties are illustrations, not universal UI requirements.
The evidence rule, both directions, value pinning, and the adversarial review
are adapted from the MIT-licensed hegel skill
(https://github.com/hegeldev/hegel-skill).
