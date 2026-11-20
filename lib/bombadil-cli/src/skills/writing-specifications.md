# Writing Bombadil specifications

Translate the application's contract into general properties, not fixed test
scripts. Let generators supply the actions and their input values; a
specification that types hand-written values in a set order is a unit test,
not a fuzzing campaign. Read existing specifications and the installed
TypeScript definitions before writing new ones. Use the project's
`@antithesishq/bombadil` package.

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

## Extract observable state

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

## Express properties with temporal operators

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
documented result. Reject: invalid or hostile input is refused visibly and
leaves state unchanged. Get such input from generators with wide lengths and
character sets, not from a hand-written list of bad values.
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

## Generate input values

Text parameters in an action template take a string generator, not a string.
Bombadil draws a new value each time it picks the action:

- `"Email"`: an email address.
- `{ Text: [1, 100] }`: Unicode text with a length in that range.
- `{ Regexp: "[0-9]{1,5}" }`: a string matching the expression, in Rust
  `regex` syntax.
- `{ CharSet: entries }`: text drawn from character ranges and literal strings.
  Build the entries with `CharSet.fromRange`, `CharSet.fromLiterals`, and
  `CharSet.union` from `@antithesishq/bombadil/actions`. The terminal defaults
  export ready-made sets as `CharSets`.

```typescript
// Browser: the generator goes under `text`.
{ TypeText: { text: { Regexp: "[0-9]{1,5}" } } }
// Terminal: the generator is the value itself.
{ TypeText: { CharSet: CharSet.fromLiterals("\x1b", "\r", "q") } }
```

Numeric parameters, such as the row and column of a terminal `Click`, take a
`[min, max]` range in the same way.

Generate by default. A hand-written value covers the one case you thought of.
Do not script a fixed sequence of steps either: return the actions possible in
the current state and let Bombadil order them. Write properties that hold for
whatever value was generated, not for one particular input.

A literal needs a reason, stated in a comment. Two are valid: a value the
application requires to get past a gate, such as credentials or consent, and a
known boundary value added as one `weighted` branch beside the generated one,
never instead of it.

## Add custom browser actions

Browser custom actions use `registerCustomAction(name, handler)`. The handler
receives `Document`, `Window`, and custom arguments. Return calls to the
registered action from a guarded action generator. This can handle workflows
such as authentication or consent, which need fixed values; keep those values
to what the gate requires and leave everything behind it to the generators.
Terminal custom actions are not supported; use terminal action templates
instead.

## Validate and exercise

Check imports and types against the project's installed package and use its
existing typecheck command. Run a short bounded `test` to check setup, then
prefer a bounded fuzzing campaign to find bugs:

```sh
bombadil browser test --headless --time-limit 30s --output-path browser-spec-check.bin http://localhost:3000 spec.ts
bombadil terminal test --specification tui.spec.ts --time-limit 30s --output-path terminal-spec-check.bin -- your-tui
bombadil skill show fuzzing
```

Check in the trace that the states each property watches are reached. Do not
weaken, unexport, or delete a property to make a violation disappear. Use
`bombadil skill show investigating-failures` to distinguish application
defects from specification and setup errors.

## Review the specification

Check every exported property and action generator against this list, for a
specification you wrote or one you were asked to review. When a check fails,
quote the offending code and give the smallest change that fixes it. A
specification can pass every run and still check nothing; most items below are
ways that happens.

1. **Evidence-free property**: it asserts behavior that nothing documents or
   implies, or restates the implementation. Each property should name the
   contract it checks: documentation, visible UI text, an existing test, or an
   ordinary expectation such as "a saved item appears in the list".
2. **Vacuous implication**: the precondition of `.implies` never held in the
   runs. Check the traces for states where the trigger is true. An implication
   whose trigger was never reached has not been tested.
3. **Unreachable trigger**: no exported action generator can produce the state
   the property watches, such as a form that needs a login the actions cannot
   perform. Add a custom action or a guarded generator that reaches it.
4. **Validity-only oracle**: the property checks only that something exists,
   is nonempty, or shows no error, where the value could be pinned cheaply. A
   counter should change by exactly the number of increments; a cart total
   should equal the sum of its lines; a submitted name should appear in the
   notification.
5. **Missing reject direction**: input handling is checked only with valid
   input. Invalid input should be rejected visibly and leave state unchanged.
6. **Narrowed actions**: `filter`, `when`, weights, or `--allow-url` were
   tightened to avoid a violation, or an action was removed after it failed.
   Restrict actions only to keep the run safe or inside the application, and
   say why in a comment.
7. **Restriction defeated by a wildcard export**: the module restricts a
   default generator but also has `export *` from the driver defaults. The
   browser defaults export the unrestricted `defaultActions`, so both run.
   Export the default properties by name and the restricted generator alone.
8. **Lost defaults**: a custom module exports neither the driver defaults nor
   named default properties, so checks such as uncaught exceptions or
   successful exit no longer run.
9. **Guarantee without a deadline**: an `eventually` with no `.within` bound.
   Give each guarantee the deadline the contract implies, as the manual's
   guarantee examples do.
10. **Cell read outside a thunk**: `.current` is read at module top level or
    outside the formula's thunk, so the property sees one fixed value.
11. **Acting extractor**: an extractor clicks, types, mutates the page, or
    returns values that are not JSON-serializable. Extractors only observe.
12. **Silent-triage residue**: a property left unexported, commented out,
    deleted, or loosened after a violation without a recorded reason. An
    unexported property is not checked.
13. **One export, several contracts**: unrelated conditions joined with `.and`
    in one property. Bombadil reports violations by export name; split them
    so a violation names the broken contract.
14. **Hand-written input**: an action types a literal where a string generator
    would do, a generator scripts a fixed sequence of steps, or a property
    expects one particular input, with no reason in a comment. Use a
    generator; keep literals for gates such as login or consent.

## Before you finish

1. **Adversarial review by a subagent.** Launch a separate subagent with the
   application's contract (documentation, UI text, routes, existing tests)
   but not your specification; a reviewer holding your properties inherits
   your blind spots. Ask it to assume the application is wrong and to return
   the properties the contract implies, the inputs and action sequences most
   likely to break each, and the actions needed to reach them. Write what it
   returns as properties and actions, or state why you dropped each one.
2. **Checklist.** Check every exported property and generator against the
   review checklist above.
3. **Exercise.** Run a bounded campaign as described above and report what it
   found, which properties it reached, and which it did not.
