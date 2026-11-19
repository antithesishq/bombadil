# Reviewing Bombadil specifications

Check every exported property and action generator against this list. When a
check fails, quote the offending code and give the smallest change that fixes
it. A specification can pass every run and still check nothing; most items
below are ways that happens.

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
