# External driver protocol, version 0

An external driver lets Bombadil test an app it can't drive itself, such
as a SwiftUI, Jetpack Compose or Flutter app. Bombadil runs the driver as
a child process (`bombadil external test --specification app.spec.ts --
<driver> [args...]`), one process per test run, and the two exchange
messages as **newline-delimited JSON**:

- Bombadil writes to the driver's **stdin**.
- The driver writes to its **stdout**, and only protocol messages. Logs go
  to **stderr**, which Bombadil passes through.

Every message is an object with a `"type"` field. Bombadil sets the
environment variable `BOMBADIL_RUN_ID` to the run's ID.

## Session

```
driver → {"type":"hello","protocol":0,"name":"my-swiftui-driver"}
driver → {"type":"state","state":{...}}            initial state
Bombadil → {"type":"apply","action":{...}}
driver → {"type":"state","state":{...}}            state after the action
  ... apply / state, repeated ...
Bombadil → {"type":"terminate"}
driver exits
```

1. The driver launches the app and sends `hello` within 60 seconds.
   `protocol` must be `0`; Bombadil refuses other versions.
2. Once the app has settled, the driver sends its first `state`.
3. For every `apply`, the driver performs the action, waits for the app to
   settle, and sends exactly one `state`. Bombadil considers the driver
   stalled if no `state` arrives within 30 seconds.
4. On `terminate`, the driver shuts the app down and exits. Bombadil kills
   it if it hasn't exited after 5 seconds, and also if Bombadil itself
   stops.

If the driver can't continue, it sends `{"type":"error","message":"..."}`
and exits. Bombadil ends the run with that error. Exiting without an
error, or writing a line that isn't a valid message, also ends the run.

## Messages

### From the driver

| Message | Fields |
| --- | --- |
| `hello` | `protocol`: integer, `name`: string |
| `state` | `state`: any JSON value |
| `error` | `message`: string |

### From Bombadil

| Message | Fields |
| --- | --- |
| `apply` | `action`: any JSON value |
| `terminate` | |

## State

`state` is whatever JSON describes the app well enough for properties to
check it, typically a tree of visible views with their identifiers, roles,
labels, values and frames. Bombadil timestamps a state when it receives
it. The specification reads states with `extract` from
`@antithesishq/bombadil/external`, so its state type must match what the
driver sends.

## Actions

An action is JSON chosen by the specification. Bombadil never interprets
it, so driver and specification only need to agree on its shape. A
convention that SDKs should follow is an externally tagged object, such
as `{"Tap":{"id":"submit"}}`, `{"TypeText":{"id":"name","text":"..."}}` or
`"Back"`.

Specifications generate actions from templates, in which some leaves are
generators:

```ts
import { actions, extract, integer, string } from "@antithesishq/bombadil/external";

export const tapAndType = actions(() => [
  { Tap: { id: "submit" } },
  { Scroll: { by: integer([1, 10]) } },
  { TypeText: { id: "name", text: string({ Text: [0, 20] }) } },
]);
```

Bombadil replaces generators with values before sending `apply`, so the
driver only ever receives concrete JSON. Generation happens in Bombadil
with its own random seed, which keeps runs reproducible and shrinkable.
