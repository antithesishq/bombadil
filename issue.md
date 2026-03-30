# Fix: Implies/Or Equivalence Discrepancy in Bombadil LTL Monitor

## Problem

Bombadil preserves `Implies` as a distinct connective in NNF rather than
desugaring `phi => psi` to `!phi || psi`. This causes a semantic discrepancy:
the two forms can disagree on truth value for the same trace.

### Counterexample

```
phi = F(F(Y))         -- eventually eventually Y
psi = G(false)        -- always false
trace = [{y:true}, {y:false}]
```

| Form | NNF | Result |
|------|-----|--------|
| `phi => psi` | `Implies(F(F(Y)), G(false))` | **false** |
| `!phi \|\| psi` | `Or(G(G(!Y)), G(false))` | **true** |

Standard LTL gives `false` for both. The discrepancy is internal to
Bombadil's evaluation semantics.

### Root Cause

In `evaluate_implies`, when the antecedent evaluates to `True`, it is stored
as `Residual::True` in the continuation:

```rust
// evaluate_implies — the (True, Residual) case
(Value::True(snapshots), Value::Residual(r)) =>
    Value::Residual(Residual::Implies(formula, Residual::True(snapshots), r))
//                                             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
//                                             LOCKED — always steps to True
```

On subsequent `step` calls, `Residual::True` always evaluates to
`Value::True`. The antecedent is irrevocably decided.

In the equivalent `Or(!phi, psi)` form, what was `True` for `phi`
corresponds to `False(violation, Some(continuation))` for `!phi`. The
continuation carries the Always/Eventually machinery and can **evolve** at
subsequent steps. When `Or` steps both sides and the left (negated
antecedent) produces a `Residual` at a later state, the `Or` becomes
non-False — even though it was False at the initial evaluation.

```
Step 0:
  Implies: phi=True → lock rTrue → Implies(phi, rTrue, G(false)-cont)
  Or:      !phi=False(cont) → Or(!phi-cont, G(false)-cont)

Step 1:
  Implies: step(rTrue)=True → evaluate_implies(True, ...) → False
  Or:      step(!phi-cont)=Residual → evaluate_or(Residual, False) → Residual

  Implies stays False. Or becomes Residual (resolves to True via AssumeTrue leaning).
```

## Fix

### Approach: Re-evaluate negated antecedent during stepping

A single change to `step()` for `Residual::Implies`. When the left
(antecedent) residual is `Residual::True`, instead of stepping it (which
always returns `True`), re-evaluate `negate_formula(phi)` at the current
state and combine with `evaluate_or` semantics.

This mirrors what `Or(!phi, psi)` does structurally: the `!phi` side is
re-evaluated at each step via the Always continuation machinery.

**No other functions need to change.** `evaluate_implies`, `stop_default`,
and all other evaluation/combination functions remain untouched.

### Why `stop_default` is unaffected

At trace end, the locked antecedent resolves identically under both
semantics:

```
stop_implies(Some(true), x) = x     -- true antecedent: result is consequent
stop_or(Some(false), x)     = x     -- false !phi: result is right side
```

These are equivalent for all `x`. The fix only affects intermediate stepping.

### Proof

This fix has been **machine-checked in Lean 4**. See `Main.lean`:

- `fixed_implies_gives_true`: fixed implies form gives `some true` on the
  counterexample trace (was `some false`)
- `fixed_or_still_true`: or form is unchanged at `some true`
- `fixed_implies_or_agree`: both forms agree after the fix
- `stop_equiv_true_ante`: stop_default equivalence proven for all inputs

### Rust implementation

The change is in `step()` for the `Residual::Implies` match arm:

```rust
// In step() — before fix:
Residual::Implies(formula, left, right) => {
    let left_val = self.step(left, state);
    let right_val = self.step(right, state);
    evaluate_implies(formula, left_val, right_val)
}

// After fix:
Residual::Implies(formula, left, right) => {
    match left {
        Residual::True(_) => {
            // Re-evaluate the negation of the antecedent formula at the
            // current state. This produces the same continuation structure
            // that Or(!phi, psi) would have, keeping the antecedent alive.
            let neg_formula = negate_formula(formula);
            let neg_val = evaluate(&neg_formula, state);
            let right_val = self.step(right, state);
            evaluate_or(neg_val, right_val)
        }
        _ => {
            // Antecedent still pending — normal Implies semantics
            let left_val = self.step(left, state);
            let right_val = self.step(right, state);
            evaluate_implies(formula, left_val, right_val)
        }
    }
}
```

`negate_formula` already exists in Bombadil (it is used by the NNF
conversion). It pushes negation through the formula in NNF form:

```
negate(Pure(b))       = Pure(!b)
negate(Atom(a, neg))  = Atom(a, !neg)
negate(And(l, r))     = Or(negate(l), negate(r))
negate(Or(l, r))      = And(negate(l), negate(r))
negate(Impl(l, r))    = And(l, negate(r))
negate(Next(f, at))   = Next(negate(f), !at)
negate(Always(f))     = Eventually(negate(f))
negate(Eventually(f)) = Always(negate(f))
```

### What this costs

- One additional `evaluate()` call per step when the antecedent was True.
  This only occurs for `Implies` residuals where the left side is
  `Residual::True` — i.e., the antecedent was decided True at some prior
  step and hasn't changed since.
- The `negate_formula` call is a structural transformation (no allocation
  beyond the new formula tree). It could be cached on the `Residual::Implies`
  variant if performance matters.
- Violation reporting is unaffected. The initial `False(ImpliesViolation, ...)`
  from `evaluate_implies(True, False)` is still produced with the correct
  antecedent formula and snapshots. The fix only changes what happens to the
  continuation on subsequent steps.

### Alternative: desugar Implies to Or

A simpler but less surgical fix: desugar `Implies(phi, psi)` to
`Or(negate(phi), psi)` during NNF conversion, then wrap violations in
Implies-shaped error messages after the fact.

The Lean model proves this produces syntactically identical formulas:

```lean
theorem desugar_implies_is_or :
    desugar_implies implies_nnf = or_nnf := by native_decide
```

This guarantees semantic equivalence by construction but requires
reconstructing the Implies violation context from the Or structure for error
messages.

## Other equivalences verified

The Lean formalization also proves these NNF-level equivalences hold:

| Equivalence | Status |
|-------------|--------|
| De Morgan: `!(phi && psi) = !phi \|\| !psi` | proven (syntactic NNF identity) |
| De Morgan: `!(phi \|\| psi) = !phi && !psi` | proven (syntactic NNF identity) |
| G/F duality: `G(!phi) = !F(phi)` | proven (syntactic NNF identity) |
| F/G duality: `F(!phi) = !G(phi)` | proven (syntactic NNF identity) |
| Double negation: `!!phi = phi` | proven (syntactic NNF identity) |
| Negated implies: `!(phi => psi) = phi && !psi` | proven (syntactic NNF identity) |
| Next duality: `X(!phi) != !X(phi)` | proven **different** (leaning flags differ) |
| Propositional no-residual | proven (propositional formulas never produce Residual) |
| Implies/Or counterexample | proven (disagree on `[{y:true},{y:false}]`) |
| Fix resolves counterexample | proven (both give `some true` after fix) |
| stop_default unaffected by fix | proven (algebraic equivalence) |

## Test plan

- [ ] Apply the `step()` change to the Rust implementation
- [ ] Run existing `ltl_equivalences` proptest suite — the implies/or
      equivalence tests should now pass
- [ ] Verify the specific counterexample trace `[{y:true}, {y:false}]` with
      `phi = F(F(Y))`, `psi = G(false)`
- [ ] Check that violation error messages for Implies formulas are unchanged
      (the fix only affects continuation stepping, not initial violation
      reporting)
- [ ] Run full test suite to check for regressions
