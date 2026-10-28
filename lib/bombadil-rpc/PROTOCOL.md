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
   stalled if no `state` arrives within the stall timeout: 30 seconds, or
   `stall_timeout_ms` from `hello`. This also bounds the wait for the first
   `state`, so drivers of slow-starting apps should raise it.
4. On `terminate`, the driver shuts the app down and exits. The driver
   runs in its own process group. Once the driver has exited, or after 5
   seconds if it hasn't, Bombadil sends SIGTERM to that group, then SIGKILL
   5 seconds later, and waits until the group is gone before the next
   session starts or Bombadil exits. The same happens if a session ends
   any other way. Processes that should outlive a session, such as an
   emulator or an adb server, must leave the group (for example with
   `setsid`) or be started outside the driver.

If the driver can't continue, it sends `{"type":"error","message":"..."}`
and exits. Bombadil ends the run with that error. Exiting without an
error, or writing a line that isn't a valid message, also ends the run.

## Messages

### From the driver

| Message | Fields |
| --- | --- |
| `hello` | `protocol`: integer, `name`: string, `stall_timeout_ms`: optional integer |
| `state` | `state`: any JSON value, `settled`: optional boolean, `not_applied`: optional string, `timestamp_us`: optional integer |
| `error` | `message`: string |

### From Bombadil

| Message | Fields |
| --- | --- |
| `apply` | `action`: any JSON value |
| `terminate` | |

## State

`state` is whatever JSON describes the app well enough for properties to
check it, typically a tree of visible views with their identifiers, roles,
labels, values and frames. The specification reads states with `extract`
from `@antithesishq/bombadil/external`, so its state type must match what
the driver sends.

Besides `state`, the message says how the driver got it. Bombadil records
these fields in the trace, but specifications don't see them:

- `settled`: `false` if the driver gave up waiting for the app to settle
  (for example on a cap for endless animations) and reports the state as
  it was. Defaults to `true`.
- `not_applied`: the reason, if the driver didn't perform the action it
  was sent, for example because the node it names is gone or the system
  refused the input. The state is then the current one, unchanged by the
  action. Not allowed on the first state.
- `timestamp_us`: when the driver read the state, in microseconds since
  the Unix epoch, on the clock of the machine Bombadil runs on. Timing
  properties such as `within(...)` and time limits use it, so a driver
  that reads a device's clock must convert to the host's. It must not go
  backwards within a session. Defaults to when Bombadil receives the
  state.

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
