# Specification Language

To extend Bombadil with domain-specific knowledge, you write specifications.
These are plain TypeScript or JavaScript modules using the library provided by
Bombadil, exporting *properties* and *action generators*.

## Structure

A specification is a regular ES module. The following examples use TypeScript,
but you may also write them in JavaScript.


::: {.callout .callout-note}
If you do use TypeScript, you'll want to install the types from [@antithesishq/bombadil](2-getting-started.html#typescript-support).
:::

Both properties and action generators are exposed to Bombadil as exports:

```typescript
export const myProperty = ...; 

export const myAction = ...;
```

You may split up your specification into multiple modules and structure it the
way you like, but the top-level specification you give to Bombadil must only
export properties and action generators. 

## Defaults

Bombadil comes with a set of default properties and action generators that work
for most web applications. You'll probably want to reexport these:

```typescript
export * from "@antithesishq/bombadil/defaults";
```

In fact, this is exactly what is used when running tests without a custom
specification file. If you want to selectively pick just a subset of these,
use the following modules:

```typescript
export { 
    noUncaughtExceptions
} from "@antithesishq/bombadil/defaults/properties";
export { 
    clicks, 
    reload,
} from "@antithesishq/bombadil/defaults/actions";
```

You may freely combine defaults with your own properties and actions.

## Properties

A property is a description of how the system under test should behave *in
general*. This is different from example-based testing (Playwright, Cypress)
where you describe how it behaves for *particular* cases.

The most intuitive kind of property, which you might have come across before,
is an *invariant*. In Bombadil, invariants are expressed using the `always`
temporal operator:

```typescript
always( 
    // some condition that should always be true
)
```

To instruct Bombadil to check your property, you must export it from your
specification module. Its name is used in error reports, so give the
export a meaningful name.

```typescript
export const pageHasTitle = always( 
    // check that there's a page title somehow
);
```

You may export multiple properties, including the [defaults](#defaults), and
they'll all be checked independently. But how do you "check that there's a page
title somehow"? You need access to the browser, and for that, you use *extractors*.

## Extractors

In order to describe a condition about the web page you're testing, you first
need to extract state. This is done with the `extract` function, which runs
inside the browser on every state that Bombadil decides to capture.

```typescript
extract(state => ...)
```

You give it a function that takes the current browser state as an argument, and
returns JSON-serializable data. The state object contains a bunch of things,
but most important are `document` and `window`, the same ones you have access
to in JavaScript running in a browser.

To extract the page title, you'd define this at the top level of your
specification:

```typescript
const pageTitle = extract(state => state.document.title || "");
```

The `pageTitle` value is not a `string` though --- it's a `Cell<string>`, a
stateful value that changes over time. For every new state captured by
Bombadil, the extractor function gets run, and the cell is updated with its
return value.

Using the `pageTitle` cell, you can define the property:

```typescript
export const pageHasTitle = always(() => 
    pageTitle.current !== ""
);
```

There are a couple of new things going on here:

1. The expression passed to `always` is a function that takes no arguments ---
   a *thunk*. This is because it needs to be evaluated in every state. It needs
   to *always* be true, not just once, and that's why you need to supply the
   thunk rather than a `boolean`.
2. To get the `string` value out of the cell, you use `pageTitle.current`.

This is a custom property using the *temporal* operator called `always`.
There are other temporal operators, described in [Formulas](#formulas).

## Formulas

Formulas and temporal operators may sound scary, but fear not --- they are
essentially ways of expressing "conditions over time". Here are some quick
facts about formulas and temporal operators:

* Temporal operators return formulas. 
* Every property in Bombadil is a formula (of the `Formula` type). 
* A temporal operator is a function that takes some subformula and evaluates it
  over time. 
* Different temporal operators evaluate their subformulas in different ways.
* Bombadil evalutes formulas against a sequence of states to check if they *hold true*.

In addition to `always`, there's also `eventually` and `next`. Here's an
informal[^ltl] description of how they work:

* `always(x)` holds if `x` holds in *this* and *every future* state
* `next(x)` holds if `x` holds in *the next* state
* `eventually(x)` holds if `x` holds in *this* or *any future* state

Remember that they accept *subformulas* as arguments. But in the example with
`always` above, the argument was a thunk. This works because the operators
automatically convert thunks into formulas. There's an operator for doing that
explicitly, called `now`:

```typescript
always(now(() => pageTitle.current !== ""))
```

You normally don't have to use the `now` operator, unless you want to use
*logical connectives* at the formula level. They are defined as methods on
formulas:

* `x.and(y)` holds if `x` holds and `y` holds
* `x.or(y)` holds if `x` holds or `y` holds
* `x.implies(y)` holds if `x` doesn't hold or `y` holds

There's also negation, both as a function and as a method on
formulas, i.e. `not(x)` and `x.not()`.

The `now` operator is useful when expressing single-state preconditions. The
following property checks that pressing a button shows a spinner that is
eventually hidden again:

```typescript
now(() => buttonPressed).implies(
    now(() => spinnerVisible).and(eventually(() => !spinnerVisible))
)
```

You can build more advanced formulas, even with nested temporal operators, but
the basics are often powerful enough. See the [examples](#examples) at the bottom for more
inspiration.

## Actions

In addition to exporting properties in specification, you export action
generators. A generator in an object with a `generate()` method. An action
generator is such an object that generates values of type `Tree<Action>`.

**TODO:** link to `Action` type when we have generated TypeScript reference

Like with [default properties](#defaults), there are default actions provided
by Bombadil. These will get you a long way, but there are times where you
need to define your own action generators.

For every state that Bombadil captures, all action generators are run, contributing
to a tree structure of *possible* actions. Bombadil then randomly picks one in that
tree. Why a tree, though? It's because the branches are *weighted* --- by default
they're equally weighted, but you can override this to control the probability of
an action being picked.

To define a custom action, you use the `actions` function, which takes a thunk
that returns an array of actions:

```typescript
export const myAction = actions(() => {
  return [
    ...
  ];
});
```

The actions you return must be possible to perform in the current state. Your
action generators should therefor depend on [cells](#extractors) and validate
your actions before returning them. As an example, the `back` action generator
provided by Bombadil checks that there's a history entry to go back to, otherwise
it returns `[]`.

## Examples

**TODO:** Example specifications

[^ltl]: Formally, the properties in Bombadil use a flavor of
[Linear Temporal Logic](https://en.wikipedia.org/wiki/Linear_temporal_logic), if you're into
dense theoretical stuff. 
