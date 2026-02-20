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

You may freely combine defaults with your own properties and actions --- you'll
learn more about this in the next section.

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
they'll all be checked independently. But how do we "check that there's a page
title somehow"? We need access to the browser, and for that, we use *extractors*.

## Extractors

In order to describe a condition about the web page you're testing, you first
need to extract state. This is done with the `extract` function, which runs
inside the browser on every state that Bombadil decides to capture.

```typescript
extract(state => ...)
```

You give it a function that takes the current browser state as an argument, and
returns some JSON-serializable data. The state object contains a bunch of
things, but for now we'll focus on the `document` and `window`, which are the
same ones you have access to in JavaScript running in a browser.

To extract the page title, you'd define this at the top level of your
specification:

```typescript
const pageTitle = extract(state => state.document.title || "");
```

The `pageTitle` value is not a `string` though --- it's a `Cell<string>`, a
stateful value that changes over time. For every new state captured by
Bombadil, the extractor function gets run, and the cell is updated with its
return value.

Using the `pageTitle` cell, you can now define the property:

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

You know have a custom property using the *temporal* operator called `always`.
There are other temporal operators that you'll learn about in the next section.

## Temporal Operators

## Actions

TODO: Available actions

## Examples

TODO: Example specifications
