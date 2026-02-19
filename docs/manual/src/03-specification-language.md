# Specification Language

foo bar baz.

## Overview

TODO: Overview of the specification language

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
