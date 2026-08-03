# BombadilAgent

> [!WARNING]
> The SwiftUI driver is experimental, macOS-only for now, and its
> protocol may change between Bombadil versions.

The in-app agent that lets [Bombadil](https://github.com/antithesishq/bombadil)
fuzz-test SwiftUI apps. Bombadil's `swiftui` driver launches your app,
and this agent connects back to it over a local TCP socket, streaming
the accessibility tree as the observable state and applying generated
actions (taps, typed text, key presses, scrolling) as in-process
events — no system accessibility permissions required.

## Integrating

Add this package as a dependency of your app target, then start the
agent early in the app's lifecycle:

```swift
import BombadilAgent

@main
struct MyApp: App {
    init() {
        #if DEBUG
        BombadilAgent.startIfRequested()
        #endif
    }
    ...
}
```

`startIfRequested()` is a no-op unless the app was launched by
`bombadil swiftui test` (detected via the `BOMBADIL_SWIFTUI_CONNECT`
environment variable). Linking it only into development builds keeps
the test-control surface out of production binaries.

Since the state is the accessibility tree, the more accessible your
app is, the better Bombadil can test it: `accessibilityIdentifier`
gives your specification stable handles on elements, and
`accessibilityValue` exposes state to properties.

## Running a test

The macOS-only CLI command is opt-in. Build Bombadil from source with
`cargo build -p bombadil-cli --features swiftui` to enable it.

```sh
# Launch the app yourself (e.g. from Xcode) and attach:
bombadil swiftui test --attach

# Or let Bombadil launch it:
bombadil swiftui test -- ./MyApp.app/Contents/MacOS/MyApp

# With a specification:
bombadil swiftui test --specification spec.ts --exit-on-violation -- ...
```

Specifications use the `@antithesishq/bombadil/swiftui` API. See
`examples/swiftui_counter.ts` in the Bombadil repository.

## The example app

`CounterExample` is a deliberately buggy counter app used to try out
the driver end to end:

```sh
swift run CounterExample &   # ...or let bombadil launch it:
bombadil swiftui test \
    --specification examples/swiftui_counter.ts \
    --exit-on-violation \
    -- .build/debug/CounterExample
```

## Protocol

One JSON document per line over TCP; the driver listens, the agent
connects. Kept in sync with `lib/bombadil-swiftui/src/agent.rs`:

* agent → driver: `{"type": "hello", "protocolVersion": 1}` once, then
  replies: `{"type": "state", "root": <node tree>}`,
  `{"type": "applied"}`, `{"type": "error", "message": "..."}`.
* driver → agent: `{"type": "getState", "quiescenceMillis": 100}`,
  `{"type": "apply", "action": {"Tap": {"x": 10, "y": 20}}}`.

Node frames and action points use screen coordinates in points with
the origin at the top-left.
