# Specification Language

To extend Bombadil with domain-specific knowledge, you write specifications.
These are plain TypeScript or JavaScript modules using the library provided by
Bombadil, exporting *properties* and *action generators*.

## Overview

A specification is a regular ES module. In the following examples we will use
TypeScript, but you may also write them in JavaScript.


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

```typescript
// Here is a property.
export const noHttpErrorCodes = always(
  () => (responseStatus.current ?? 0) < 400,
);
```

## Actions

TODO: Available actions

## Examples

TODO: Example specifications
