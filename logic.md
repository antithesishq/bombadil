# Bombadil LTL Runtime Monitor: Formal Specification

This document specifies the semantics of Bombadil's LTL runtime monitor precisely
enough to model in a proof assistant (Lean 4, Coq, Agda, etc.) and prove or
disprove key equivalences.

## 1. Syntax

### 1.1 Surface Syntax

```
Syntax ::= Pure(b, s)           -- boolean literal b with pretty-print string s
         | Thunk(f)             -- opaque callback f
         | ¬ Syntax             -- negation
         | Syntax ∧ Syntax      -- conjunction
         | Syntax ∨ Syntax      -- disjunction
         | Syntax ⇒ Syntax      -- implication
         | X Syntax             -- next
         | G Syntax             -- always (unbounded)
         | G[d] Syntax          -- always (bounded by duration d)
         | F Syntax             -- eventually (unbounded)
         | F[d] Syntax          -- eventually (bounded by duration d)
```

### 1.2 Internal Formula (NNF)

Negation Normal Form pushes negation to leaves. Implication is *preserved*
(not desugared) for better error messages. This is the key design choice
that creates the equivalence question.

```
Formula ::= Pure(b, s)
          | Thunk(f, negated)
          | Formula ∧ Formula
          | Formula ∨ Formula
          | Formula ⇒ Formula           -- preserved, not desugared
          | X(Formula, assume_true)      -- next with leaning direction
          | G(Formula, bound)            -- always
          | F(Formula, bound)            -- eventually
```

### 1.3 NNF Conversion

```
nnf(φ, neg) where neg ∈ {true, false}:

  nnf(Pure(b, s), neg)       = Pure(neg ? ¬b : b, s)
  nnf(Thunk(f), neg)         = Thunk(f, neg)
  nnf(¬ φ, neg)              = nnf(φ, ¬neg)

  nnf(φ ∧ ψ, false)          = nnf(φ, false) ∧ nnf(ψ, false)
  nnf(φ ∧ ψ, true)           = nnf(φ, true)  ∨ nnf(ψ, true)       -- De Morgan

  nnf(φ ∨ ψ, false)          = nnf(φ, false) ∨ nnf(ψ, false)
  nnf(φ ∨ ψ, true)           = nnf(φ, true)  ∧ nnf(ψ, true)       -- De Morgan

  nnf(φ ⇒ ψ, false)          = nnf(φ, false) ⇒ nnf(ψ, false)     -- PRESERVED
  nnf(φ ⇒ ψ, true)           = nnf(φ, false) ∧ nnf(ψ, true)      -- ¬(φ⇒ψ) = φ∧¬ψ

  nnf(X φ, neg)               = X(nnf(φ, neg), ¬neg)               -- leaning = ¬neg
  nnf(G φ, false)             = G(nnf(φ, false), bound)
  nnf(G φ, true)              = F(nnf(φ, true), bound)             -- ¬G = F¬
  nnf(F φ, false)             = F(nnf(φ, false), bound)
  nnf(F φ, true)              = G(nnf(φ, true), bound)             -- ¬F = G¬
```

**Observation (Next leaning):** A user-written `X φ` enters NNF with `neg=false`,
producing `X(nnf(φ,false), true)` — AssumeTrue leaning. A negated `¬X φ` enters
with `neg=true`, producing `X(nnf(φ,true), false)` — AssumeFalse leaning. This
mirrors how `G` and `F` swap under negation with opposite leanings.

**Observation (Implies preservation):** `nnf(φ ⇒ ψ, false)` preserves the `⇒`
constructor. The logically equivalent `nnf(¬φ ∨ ψ, false)` produces `∨` instead.
These two formulas are evaluated by *different* code paths (`evaluate_implies` vs
`evaluate_or`), which is the source of the equivalence question.


## 2. Semantic Domain

Time is modeled as a natural number (or `SystemTime`; only ordering matters).

### 2.1 Values

Evaluation of a formula at a point in time produces a `Value`:

```
Value ::= True(snapshots)
        | False(violation, continuation?)
        | Residual(residual)
```

- `True(snapshots)` — the formula is satisfied; snapshots are witness data.
- `False(violation, continuation?)` — the formula is **definitively** violated;
  the violation describes why. The optional `continuation` is a residual that
  allows monitoring to continue in enclosing contexts like `Always`, so that
  multiple violations can be collected over the trace. The `False` itself is
  final — the continuation does not "undo" the violation. It exists so that
  `Always(φ)` can report "φ failed at t₃" and then keep checking φ at t₄, t₅,
  etc. to find additional failures.
- `Residual(residual)` — the formula's truth value depends on future states.

### 2.2 Residuals

A residual represents a pending computation awaiting the next state:

```
Residual ::= True(snapshots)                         -- resolved true
           | False(violation)                        -- resolved false
           | Derived(derived, leaning)               -- temporal operator pending
           | And(residual, residual)
           | Or(residual, residual)
           | Implies(formula, residual, residual)    -- carries antecedent formula
           | AndAlways(formula, start, end?, residual, residual)
           | OrEventually(formula, start, end?, residual, residual)
```

### 2.3 Derived (Temporal Operators)

```
Derived ::= Once(start, formula)                     -- Next: evaluate once at next step
          | Always(start, end?, formula)             -- G: re-evaluate at each step
          | Eventually(start, end?, formula)         -- F: re-evaluate at each step
```

### 2.4 Leaning

When a trace ends and a residual has not resolved, the `Leaning` determines
the default truth value:

```
Leaning ::= AssumeTrue
           | AssumeFalse(violation)
```

Leanings per operator:
- `G(φ)` leans `AssumeTrue` — "nothing went wrong yet"
- `F(φ)` leans `AssumeFalse(Eventually violation)` — "never happened"
- `X(φ, true)` (user-written Next) leans `AssumeTrue`
- `X(φ, false)` (negated Next) leans `AssumeFalse`


## 3. Evaluation Rules

### 3.1 `evaluate(formula, time) → Value`

```
evaluate(Pure(true, s), t)   = True([])
evaluate(Pure(false, s), t)  = False(FalseViolation(t, s, []), None)

evaluate(Thunk(f, neg), t)   = let (formula', snapshots) = call(f, neg)
                                in attach_snapshots(evaluate(formula', t), snapshots)

evaluate(φ ∧ ψ, t)           = evaluate_and(evaluate(φ, t), evaluate(ψ, t))
evaluate(φ ∨ ψ, t)           = evaluate_or(evaluate(φ, t), evaluate(ψ, t))
evaluate(φ ⇒ ψ, t)           = evaluate_implies(φ, evaluate(φ, t), evaluate(ψ, t))

evaluate(X(φ, assume_true), t) =
    let leaning = if assume_true then AssumeTrue
                  else AssumeFalse(FalseViolation(t, "next (test ended)", []))
    in Residual(Derived(Once(t, φ), leaning))

evaluate(G(φ, bound), t) = evaluate_always(φ, t, end_time(t, bound), t)
evaluate(F(φ, bound), t) = evaluate_eventually(φ, t, end_time(t, bound), t)
```

### 3.2 `evaluate_and(left, right) → Value`

```
evaluate_and(True(s₁), True(s₂))             = True(s₁ ++ s₂)

evaluate_and(True(s), Residual(r))            = Residual(And(True(s), r))
evaluate_and(Residual(r), True(s))            = Residual(And(r, True(s)))

evaluate_and(True(_), v@False(_, _))          = v                -- short-circuit
evaluate_and(v@False(_, _), True(_))          = v                -- short-circuit

evaluate_and(False(v₁, c₁), False(v₂, c₂))  = False(AndViolation(v₁, v₂),
                                                       combine(c₁, c₂, And))

evaluate_and(Residual(r), False(v, c))        = False(v, Some(combine_or_just(r, c, And)))
evaluate_and(False(v, c), Residual(r))        = False(v, Some(combine_or_just(r, c, And)))

evaluate_and(Residual(l), Residual(r))        = Residual(And(l, r))
```

**Important asymmetry:** `evaluate_and(True(s), Residual(r))` produces
`Residual(And(True(s), r))` — the `True(s)` is embedded as `Residual::True(s)`,
preserving snapshot data. But `evaluate_and(True(_), False(v, c))` discards the
`True` entirely and returns the `False` as-is. This means True is "absorbed"
differently depending on whether the other side is resolved or pending.

### 3.3 `evaluate_or(left, right) → Value`

```
evaluate_or(True(s₁), True(s₂))              = True(s₁ ++ s₂)
evaluate_or(True(s), _)                       = True(s)          -- short-circuit
evaluate_or(_, True(s))                       = True(s)          -- short-circuit

evaluate_or(False(v₁,c₁), False(v₂,c₂))     = False(OrViolation(v₁,v₂),
                                                       combine(c₁, c₂, Or))

evaluate_or(v, False(_, _))                   = v                -- keep non-false
evaluate_or(False(_, _), v)                   = v                -- keep non-false

evaluate_or(Residual(l), Residual(r))         = Residual(Or(l, r))
```

### 3.4 `evaluate_implies(φ, left, right) → Value`

This is the critical function. `φ` is the antecedent *formula* (kept for
violation messages).

```
evaluate_implies(φ, False(_, _), _)           = True([])         -- vacuously true

evaluate_implies(φ, True(sₗ), False(v, c))   =
    False(ImpliesViolation(φ, v, sₗ),
          c.map(|c| Implies(φ, True(sₗ), c)))                   -- ★ KEY CASE

evaluate_implies(φ, True(sₗ), True(sᵣ))      = True(sₗ ++ sᵣ)

evaluate_implies(φ, True(sₗ), Residual(r))   =
    Residual(Implies(φ, True(sₗ), r))                           -- ★ KEY CASE

evaluate_implies(φ, Residual(_), True(s))     = True(s)

evaluate_implies(φ, Residual(l), False(v, _)) =
    Residual(Implies(φ, l, False(v)))

evaluate_implies(φ, Residual(l), Residual(r)) =
    Residual(Implies(φ, l, r))
```

**Critical observation — the locking problem:**

In the case `evaluate_implies(φ, True(sₗ), Residual(r))`, the antecedent is
locked as `Residual::True(sₗ)` in the continuation. On future `step` calls,
`Residual::True(sₗ)` always evaluates to `Value::True(sₗ)` — it can never
become False. The antecedent has been *irrevocably decided*.

In the equivalent `Or(¬φ, ψ)` formulation, what was `True(sₗ)` for the
antecedent φ would correspond to `False(_, continuation)` for `¬φ`. If this
False has a continuation (e.g., from an Always that failed but can restart),
that continuation *can evolve* at the next step.

This is the fundamental asymmetry: **Implies locks a True antecedent; Or
preserves the continuation of a False ¬φ.**

### 3.5 `evaluate_always(φ, start, end, time) → Value`

Always evaluates its subformula and wraps the result:

```
evaluate_always(φ, start, end, t) =
    if end < t then True([])                      -- bound expired
    else
      let always_residual = Derived(Always(start, end, φ), AssumeTrue)
      in match evaluate(φ, t) with
        | True(_)      → Residual(always_residual)
        | False(v, c)  → False(AlwaysViolation(v, φ, start, end, t),
                               Some(wrap(c, always_residual)))
        | Residual(r)  → Residual(AndAlways(φ, start, end, r, always_residual))
```

where `wrap(c, always) = AndAlways(φ, start, end, c, always)` if c is Some,
otherwise just `always`.

**Continuation for continued monitoring:** When Always's subformula fails
(`False(v, c)`), it produces a continuation that includes a *fresh*
`Derived(Always(...), AssumeTrue)`. The violation is recorded immediately, but
the continuation allows monitoring to proceed: the Always will re-evaluate its
subformula at subsequent steps, collecting additional violations if they occur.
This is the mechanism by which `G(φ)` can report "φ failed at t₁, and again at
t₃" rather than stopping at the first failure.

### 3.6 `evaluate_eventually(φ, start, end, time) → Value`

```
evaluate_eventually(φ, start, end, t) =
    if end < t then False(EventuallyViolation(φ, TimedOut(t)), None)
    else
      let ev_residual = Derived(Eventually(start, end, φ),
                                AssumeFalse(EventuallyViolation(φ, TestEnded)))
      in match evaluate(φ, t) with
        | True(s)      → True(s)
        | False(_, _)  → Residual(ev_residual)        -- ignore failure, keep trying
        | Residual(r)  → Residual(OrEventually(φ, start, end, r, ev_residual))
```

### 3.7 `step(residual, time) → Value`

Steps a residual forward by one state:

```
step(True(s), t)              = True(s)
step(False(v), t)             = False(v, None)

step(Derived(Once(_, φ), _), t)           = evaluate(φ, t)
step(Derived(Always(s, e, φ), _), t)      = evaluate_always(φ, s, e, t)
step(Derived(Eventually(s, e, φ), _), t)  = evaluate_eventually(φ, s, e, t)

step(And(l, r), t)            = evaluate_and(step(l, t), step(r, t))
step(Or(l, r), t)             = evaluate_or(step(l, t), step(r, t))
step(Implies(φ, l, r), t)     = evaluate_implies(φ, step(l, t), step(r, t))

step(AndAlways(φ, s, e, l, r), t) =
    evaluate_and_always(φ, s, e, t, step(l, t), step(r, t))

step(OrEventually(φ, s, e, l, r), t) =
    evaluate_or_eventually(φ, s, e, t, step(l, t), step(r, t))
```

**Note:** `step` on `Derived` discards the leaning — leanings are only used by
`stop_default`, never during normal stepping.

### 3.8 `evaluate_and_always(φ, start, end, time, left, right) → Value`

This handles the conjunction of a subformula result with an Always continuation:

```
evaluate_and_always(φ, start, end, t, left, right) =
    if end < t then True([])
    else match (left, right) with
      | (True(_), True(_))             → True([])
      | (Residual(l), True(_))         → Residual(AndAlways(φ, s, e, l, True([])))
      | (True(_), Residual(r))         → Residual(AndAlways(φ, s, e, True([]), r))
      | (Residual(l), Residual(r))     → Residual(AndAlways(φ, s, e, l, r))
      | otherwise (at least one False) →
          let always_residual = Derived(Always(s, e, φ), AssumeTrue)
              inner = combine_pending(left, right, And)
              continuation = wrap(inner, always_residual)
              violation = extract_violation(left, right)
          in False(AlwaysViolation(violation, φ, s, e, t), Some(continuation))
```

### 3.9 `evaluate_or_eventually(φ, start, end, time, left, right) → Value`

```
evaluate_or_eventually(φ, start, end, t, left, right) =
    if end < t then False(EventuallyViolation(φ, TimedOut(t)), None)
    else match (left, right) with
      | (True(s), _)                    → True(s)
      | (_, True(s))                    → True(s)
      | (False(_), False(r, _))         → False(r, None)
      | (False(_), Residual(r))         → Residual(r)
      | (Residual(r), False(_))         → Residual(r)
      | (Residual(l), Residual(r))      → Residual(OrEventually(φ, s, e, l, r))
```


## 4. Stop Default (Trace Termination)

When the trace ends and residuals remain, `stop_default` resolves them using
leanings:

```
stop_default(True(s))                   = Some(True(s))
stop_default(False(v))                  = Some(False(v))
stop_default(Derived(_, leaning))       = match leaning with
                                            | AssumeTrue       → Some(True([]))
                                            | AssumeFalse(v)   → Some(False(v))

stop_default(And(l, r))                 = stop_and(stop_default(l), stop_default(r))
stop_default(Or(l, r))                  = stop_or(stop_default(l), stop_default(r))
stop_default(Implies(φ, l, r))          = stop_implies(φ, stop_default(l), stop_default(r))
stop_default(AndAlways(φ,s,e, l, r))    = stop_and_always(φ,s,e, stop_default(l), stop_default(r))
stop_default(OrEventually(_, l, r))     = stop_or_eventually(stop_default(l), stop_default(r))
```

where `stop_and`, `stop_or`, `stop_implies` mirror the logic of `evaluate_and`,
`evaluate_or`, `evaluate_implies` but on the two-valued `{True, False}` domain.


## 5. The Equivalence Problem

### 5.1 Property to Prove or Disprove

For all formulas φ, ψ, all traces σ = [s₀, s₁, ..., sₙ], and all evaluation
contexts:

> **`eval(φ ⇒ ψ, σ)` and `eval(¬φ ∨ ψ, σ)` should agree on their truth value**

where `eval(formula, σ)` means:
1. Convert to NNF: `f = nnf(formula, false)`
2. Evaluate: `v₀ = evaluate(f, t₀)`
3. For each subsequent state, extract residual and step:
   `vᵢ = step(residual(vᵢ₋₁), tᵢ)`
4. If the final value is `Residual(r)`, resolve via `stop_default(r, tₙ)`.
5. The "truth value" is: True, False, or Unresolved (if stop_default returns None).

"Agree" means both are True, both are False, or both are Unresolved. (We do NOT
require identical violations — only the same truth polarity.)

### 5.2 NNF Forms of the Two Sides

For `φ ⇒ ψ`:
```
nnf(φ ⇒ ψ, false) = Implies(nnf(φ, false), nnf(ψ, false))
```

For `¬φ ∨ ψ`:
```
nnf(¬φ ∨ ψ, false) = Or(nnf(¬φ, false), nnf(ψ, false))
                     = Or(nnf(φ, true), nnf(ψ, false))
```

### 5.3 Known Failure Case

**Input:**
- φ = `Eventually(Eventually(Y))` where Y is an atomic proposition
- ψ = `Always(Not(Pure(true)))` which reduces to `Always(Pure(false))`
- σ = `[{y: true}, {y: false}]`

**Implies form:** `Implies(F(F(Y)), G(false))`
- At t₀: left = `evaluate(F(F(Y)))` → Y is true at t₀, so F(Y) is True at t₀,
  so F(F(Y)) is True. Right = `evaluate(G(false))` → False with Always
  continuation.
- Result: `evaluate_implies(φ, True([]), False(v, Some(c)))` =
  `False(ImpliesViolation, Some(Implies(φ, True([]), c)))`
- At t₁: step on continuation. Left is `Residual::True([])` → `Value::True([])`.
  **The antecedent is locked as True forever.** Right steps the Always
  continuation, which fails again.
- Final: `False`.

**Or form:** `Or(G(G(¬Y)), G(false))`
(Because `¬F(F(Y)) = G(¬F(Y)) = G(G(¬Y))`)
- At t₀: Left = `evaluate(G(G(¬Y)))`. Inner: `G(¬Y)` evaluates ¬Y at t₀;
  Y=true so ¬Y=false → `G(¬Y)` = False with Always continuation.
  Outer G wraps: False with Always continuation (containing inner Always).
  Right = `evaluate(G(false))` → False with Always continuation.
- Result: `evaluate_or(False(_, Some(c_left)), False(_, Some(c_right)))` =
  `False(_, Some(Or(c_left, c_right)))`.
- At t₁: Step both continuations. Left: the outer Always's continuation carries
  a fresh `Derived(Always(...))` which re-evaluates `G(¬Y)` at t₁. Now Y=false,
  so ¬Y=true → inner G produces Residual (pending, subformula satisfied so far).
  Outer G wraps → Residual. So left becomes `Residual(...)`.
  Right: still False. `evaluate_or(Residual(_), False(_))` = `Residual(...)`.
- Final: `Residual` → `stop_default` → `True` (Always leans AssumeTrue).

Note: the outer Always violation at t₀ is *not undone*. It was already reported.
But the continuation — which exists for continued monitoring — happens to produce
a Residual at t₁, and the Or sees (Residual, False) and keeps the Residual side.
The Always continuation was designed for collecting further violations, but when
composed inside Or, it has the side effect of allowing the Or to become non-False.

**Discrepancy:** Implies gives `False`. Or gives `True`.

### 5.4 Root Cause Analysis

The core issue is a structural mismatch between how Implies and Or handle their
sub-values over time.

1. **Implies locks the antecedent.** When `evaluate_implies` sees `(True, _)`,
   the antecedent is stored as `Residual::True(snapshots)` in the continuation.
   This is a terminal value — it always steps to `True` and can never change.
   The antecedent's truth value is fixed for all subsequent monitoring steps.

2. **Or keeps both sides alive via continuations.** In the Or form, `¬φ` is
   evaluated as a formula in its own right. When `¬φ` evaluates to
   `False(violation, Some(continuation))`, the continuation exists for continued
   monitoring (collecting violations in enclosing Always contexts). But when Or
   composes two False continuations into `Or(c_left, c_right)` and steps them,
   each side is re-evaluated independently. If the conditions for `¬φ` change
   at a later state, its continuation (which carries the full Always machinery)
   re-evaluates and may produce Residual or True — causing the Or to become
   non-False.

3. **The asymmetry.** The continuation on `False` was designed for continued
   violation monitoring, not for changing a formula's truth value. But when
   Always wraps its continuation with a fresh `Derived(Always(...))`, and that
   continuation is composed inside an Or, the re-evaluation at subsequent steps
   can produce a different truth value for the *Or* — even though each
   individual Always failure is final. In the Implies form, φ being True means
   `¬φ` would be False-with-continuation, but Implies never sees `¬φ` — it
   sees φ = True and locks it. The continuation that would have allowed the
   Or-form's monitoring to "discover" that `¬φ` becomes True at a later state
   simply does not exist in the Implies form.

### 5.5 Precise Question

Is there a modification to `evaluate_implies` (or the continuation structure)
such that:

1. `eval(φ ⇒ ψ, σ)` agrees with `eval(¬φ ∨ ψ, σ)` for all φ, ψ, σ
2. Implies violations still carry the antecedent formula and snapshots for
   error reporting
3. The evaluator remains incremental (processes one state at a time)

### 5.6 Potential Fix Direction

The Implies evaluator needs to keep the antecedent "alive" in its continuation,
mirroring what Or does structurally. Possible approaches:

1. **Re-evaluate the antecedent formula.** Instead of storing
   `Residual::True(snapshots)` as the locked antecedent, store a residual that
   re-evaluates the original antecedent formula at each step. This is
   equivalent to what Or does: in `Or(¬φ, ψ)`, the `¬φ` side is never
   collapsed — it is stepped as a live residual.

2. **Desugar Implies to Or internally.** Evaluate `φ ⇒ ψ` as `¬φ ∨ ψ`
   internally but wrap violations in `Implies`-shaped error messages after the
   fact. This guarantees semantic equivalence but may complicate violation
   reporting.

3. **Track the antecedent's continuation separately.** When the antecedent
   evaluates to `True` but came from a formula with temporal operators, keep
   the antecedent's continuation (if any) alive alongside `True`. This requires
   a richer `Value` type or a new Residual variant.

The fundamental tension: Implies wants to *decide* the antecedent (True or
False) so it can produce clean violation messages. But a "decided True"
antecedent that came from temporal evaluation (e.g., `F(F(Y))` that happened
to be satisfied at this step) is not the same as a "definitively True"
antecedent (e.g., `Pure(true)`) — the temporal one might evaluate differently
at the next step if the antecedent formula were re-evaluated.

### 5.7 Other Equivalences to Verify

Beyond `(φ ⇒ ψ) ⇔ (¬φ ∨ ψ)`, a complete formalization should also verify:

- **De Morgan:** `¬(φ ∧ ψ) ⇔ (¬φ ∨ ¬ψ)` and `¬(φ ∨ ψ) ⇔ (¬φ ∧ ¬ψ)`
- **Next self-duality:** `X(¬φ) ⇔ ¬X(φ)` (requires ≥2 states)
- **Always/Eventually duality:** `G(¬φ) ⇔ ¬F(φ)`
- **Distributivity:** `X(φ∨ψ) ⇔ X(φ)∨X(ψ)`, `G(φ∧ψ) ⇔ G(φ)∧G(ψ)`, etc.
- **Idempotency:** `F(F(φ)) ⇔ F(φ)` and `G(G(φ)) ⇔ G(φ)`
- **Expansion:** `F(φ) ⇔ φ ∨ X(F(φ))` and `G(φ) ⇔ φ ∧ X(G(φ))`

These are the standard LTL equivalences. The existing proptest suite covers
most of them (see `ltl_equivalences.rs`).


## 6. Formal Model Sketch (for Lean 4)

A suggested approach to modeling this in a proof assistant:

```
-- Abstract away snapshots/violations/pretty-printing; focus on truth values
inductive TruthValue where
  | true
  | false
  | residual (r : Residual)

inductive Residual where
  | resolved (b : Bool)
  | derived (d : Derived) (leaning : Bool)  -- true = AssumeTrue
  | and (l r : Residual)
  | or (l r : Residual)
  | implies (l r : Residual)
  | andAlways (sub : Formula) (l r : Residual)
  | orEventually (sub : Formula) (l r : Residual)

inductive Formula where
  | pure (b : Bool)
  | atom (a : Atom)
  | and (l r : Formula)
  | or (l r : Formula)
  | implies (l r : Formula)
  | next (sub : Formula) (assumeTrue : Bool)
  | always (sub : Formula)
  | eventually (sub : Formula)

-- The key theorem to prove or disprove:
theorem implies_or_equivalence
    (φ ψ : Formula) (σ : List State) :
    eval (Formula.implies φ ψ) σ = eval (Formula.or (negate φ) ψ) σ
```

The model should be simplified:
- Drop all `Duration`/bound handling (or keep only unbounded variants).
- Replace snapshots with unit `()`.
- Replace violations with a simple `Bool` (true = violated, false = not).
- Focus on the three-valued `{True, False, Residual}` domain and
  the `stop_default` resolution.

The minimal reproduction only requires: `Pure`, `And`/`Or`/`Implies`,
`Next`, `Always`, `Eventually`, plus `evaluate`, `step`, and `stop_default`.


## 7. Trace Semantics Reference

For comparison, standard LTL over infinite traces defines:

```
σ, i ⊨ p           iff  p holds in state σ[i]
σ, i ⊨ ¬φ          iff  σ, i ⊭ φ
σ, i ⊨ φ ∧ ψ       iff  σ, i ⊨ φ  and  σ, i ⊨ ψ
σ, i ⊨ φ ∨ ψ       iff  σ, i ⊨ φ  or   σ, i ⊨ ψ
σ, i ⊨ φ ⇒ ψ       iff  σ, i ⊭ φ  or   σ, i ⊨ ψ
σ, i ⊨ X φ         iff  σ, i+1 ⊨ φ
σ, i ⊨ G φ         iff  ∀ j ≥ i.  σ, j ⊨ φ
σ, i ⊨ F φ         iff  ∃ j ≥ i.  σ, j ⊨ φ
```

Bombadil departs from standard LTL in that:
1. Traces are finite (monitored at runtime).
2. Formulas that depend on future states produce `Residual` instead of
   blocking.
3. `stop_default` provides a "best guess" when the trace ends with unresolved
   residuals — this is an approximation, not a logical entailment.
4. `Implies` is evaluated as a distinct connective (not desugared to `¬∨`)
   for better error messages.

The question is whether departure (4) introduces semantic inconsistencies
given departures (1)-(3).
