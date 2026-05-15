# Decouple Bombadil from the Browser

## Current status (resume notes)

Committed on `decouple-browser` branch, browser path green:

- Stage 1 — Schema `TraceEntry<A, S>` + `BrowserStateSummary` + alias.
- Stage 2 — Workspace split into `bombadil-browser`; files moved via
  `git mv` so history is preserved. Insta snapshots renamed
  `bombadil__*` → `bombadil_browser__*`.
- Stage 3 — `bombadil::driver::InterfaceDriver` trait,
  `bombadil::runner::Runner<D>`; `BrowserDriver` wrapper in
  `bombadil-browser`; `bombadil_browser::runner::launch()` is the CLI
  entry. `RunStrategy` keys off `<D>`. `PropertyViolation` moved to
  `bombadil::runner` (re-exported from `bombadil_browser::trace`).
  Required a `Box::pin` around `run_test` plus a `Box::pin` around each
  in-loop trait-method await (next_event / extract_snapshots /
  verifier.step / on_new_state / pick_action) to fit the test-thread
  stack; also moved BrowserDriver's 64 KB edges map to a `Vec<u8>` on
  the heap. Tests green after these.
- Stage 4 — Per-driver TS submodules; `Specification` gains
  `runtime_module`. Caught two regressions: (a) `browser.ts` /
  `terminal.ts` both `import { ActionGenerator }` *and*
  `export { ActionGenerator } from "...actions"` produced duplicate
  `const { ActionGenerator } = require(...)` bindings → SyntaxError →
  silently unset `globalThis.__bombadilRequire` → every spec test
  failed with ReferenceError. Fix: demote the value import to a type
  import; the local code only used it for return-type annotations.
  (b) `extract_snapshots` was still calling
  `__bombadilRequire('@antithesishq/bombadil')` after the runtime
  moved to `/browser`; updated.
- Stage 4.5 — All browser-specific TS under `browser/`; symmetric
  `terminal/`. Resolver gained Node-style directory fallback. Tests
  green.

- Stage 5 — TerminalDriver restructured as a Send handle holding an
  unbounded mpsc Sender plus the existing ExtractorWorker handle. A
  TerminalWorker thread owns the !Send `Terminal<'static,'static>` and
  the PtyProcess + PtyOutput, runs its own `tokio::runtime::Builder
  ::new_current_thread()`, and pumps commands (Initiate, NextEvent,
  Apply, Terminate). `apply()` is fire-and-forget — the driver
  validates PressKey scalars locally, and any worker-side apply error
  (resize / pty-write) is stashed in `pending_error` and surfaced as a
  `DriverEvent::Error` on the next `next_event`. FIFO ordering on the
  channel keeps Apply landing before the next NextEvent. The Send bound
  on the trait's async-fn return types stayed relaxed (correct
  regardless of the pivot, since drivers may legitimately have non-Send
  futures). Also kept: `js_err` helper for Boa's !Send JsError,
  `Terminal<'static,'static>` annotation, `max_scrollback: usize`.
- Stage 6 — `bombadil-cli` already exposes `bombadil terminal test
  --spec <file> -- <program> [args...]`. No CLI change required.
- Stage 7 — `lib/bombadil-terminal/tests/smoke.rs` runs the trivial
  spec `eventually(() => screen.includes("ready"))` against
  `sh -c 'printf ready\n'`. Passes. Verified that 25/25 browser
  integration tests still pass after the refactor. Pre-existing
  `bombadil::styled::tests` snapshot failures are environment-dependent
  (owo_colors detects no TTY under captured cargo stdout) and reproduce
  on the parent commit — unrelated.

## Context

Today Bombadil is a property-based testing tool for web UIs. `lib/bombadil/` is the
core crate but it is wired through with browser-specific types: `BrowserState`,
`BrowserAction`, CDP-flavored coverage, JS edge-map instrumentation, and a
fixed `State` shape on the TypeScript side.

We have a working terminal experiment in `lib/bombadil-terminal/` that drives a
PTY + ghostty-vt instead of a browser. Right now it is a standalone fuzzer that
imports only `bombadil::tree::Tree`; it does not share the verifier, the LTL
engine, or any specification machinery.

Goal: split the core so that browser and terminal are sibling **drivers**
implementing a shared `InterfaceDriver` trait, with `bombadil` as the
driver-agnostic core (runner, verifier, tree, specification bundling). One
TypeScript package — `@antithesishq/bombadil` — with per-driver submodules.
The verifier's use of Boa stays an implementation detail and never leaks into
the driver trait.

## End-state architecture

```
lib/bombadil              ← driver-agnostic core (kept name)
                            runner, verifier, worker, tree, spec bundler/resolver,
                            generic trace, InterfaceDriver trait
lib/bombadil-browser      ← new: BrowserState, BrowserAction, CDP state machine,
                            JS coverage/instrumentation, URL domain filter,
                            geometry, Resources, browser convert.rs
lib/bombadil-terminal     ← existing: TerminalState, TerminalAction, PTY driver
                            + its own private Boa for extractors
lib/bombadil-schema       ← generic TraceEntry<A, S>; per-driver action enums
                            (BrowserAction stays in browser crate, TerminalAction
                            in terminal crate; schema crate is generic only)
lib/bombadil-cli          ← depends on bombadil + both drivers
```

TypeScript package layout — generic at the root, everything browser-
or terminal-specific under a `browser/` or `terminal/` directory:

```
specification/
  index.ts             @antithesishq/bombadil             Formula API
  internal.ts          @antithesishq/bombadil/internal    Runtime<S>, ExtractorCell<T, S>
  actions.ts           @antithesishq/bombadil/actions     generic Tree<T>, ActionGenerator<A>, makeActions/makeWeighted
  random.ts            @antithesishq/bombadil/random      generic random helpers (from/strings/...)
  browser/
    index.ts           @antithesishq/bombadil/browser     BrowserState, BrowserAction union, runtime, extract, actions()/weighted()
    defaults.ts        @antithesishq/bombadil/browser/defaults             default action+property bundle
    defaults/
      actions.ts       @antithesishq/bombadil/browser/defaults/actions     default browser action generators
      properties.ts    @antithesishq/bombadil/browser/defaults/properties  default browser invariants
  terminal/
    index.ts           @antithesishq/bombadil/terminal    TerminalState, TerminalAction union, runtime, extract, actions()/weighted()
```

Each driver submodule owns its own `runtime = new Runtime<DriverState>()` and
its own `actions()` factory. Spec authors import from the submodule that
matches the run; a spec targets one interface. Generic helpers
(`@antithesishq/bombadil`, `/actions`, `/random`, `/internal`) work for
any driver.

The bundler's resolver gains Node-style directory resolution: a specifier
like `@antithesishq/bombadil/browser` resolves first to `browser.ts`
(none, after the reorg) and falls back to `browser/index.ts`. Same for
`/terminal` and for any subpath under either.

## The `InterfaceDriver` trait

In `bombadil-core` (or top-level in `bombadil`). Boa is **not** part of the
signature — each driver owns its extractor execution.

```rust
pub trait InterfaceDriver: Send {
    type Action: Clone + Debug + Serialize + DeserializeOwned + Send + 'static;
    type State: Send + Debug + 'static;

    async fn initiate(&mut self) -> Result<()>;
    async fn terminate(self) -> Result<()>;
    async fn next_event(&mut self) -> Option<DriverEvent<Self::State>>;
    fn apply(&mut self, action: Self::Action) -> Result<()>;

    /// Run user extractors against the current state and produce snapshots.
    /// Browser dispatches to Chromium via CDP. Terminal owns a private Boa
    /// context and runs extractors there. Core never sees this distinction.
    async fn extract_snapshots(
        &self,
        state: &Self::State,
        last_action: Option<&Self::Action>,
    ) -> Result<Vec<Snapshot>>;

    /// Hook for driver-specific filtering (e.g. browser's domain check).
    fn filter_actions(
        &self,
        state: &Self::State,
        tree: Tree<Self::Action>,
    ) -> Tree<Self::Action> { tree }
}

pub enum DriverEvent<S> { StateChanged(S), Error(Arc<anyhow::Error>) }
```

`Runner` becomes `Runner<D: InterfaceDriver>`. `RunStrategy` keys off
`D::Action` and `D::State` through associated types. `VerifierWorker` is
**only** about property evaluation; nothing about it changes externally.

## Critical-file map

Move to `bombadil-browser`:
- `lib/bombadil/src/browser/` (entire dir)
- `lib/bombadil/src/browser.rs`
- `lib/bombadil/src/geometry.rs`
- `lib/bombadil/src/instrumentation/` (JS edge coverage, browser-only)
- `lib/bombadil/src/url.rs` (domain filter)
- `lib/bombadil/src/specification/convert.rs` (browser ↔ schema)
- The `JsAction::to_browser_action` half of `specification/js.rs:18-142`

Stays in `bombadil`:
- `lib/bombadil/src/runner.rs` (genericized over `D: InterfaceDriver`)
- `lib/bombadil/src/specification/{verifier,worker,domain,snapshots,bundler,resolver,result,defaults}.rs`
- The generic half of `specification/js.rs` (BombadilExports, syntax_from_value, Extractors)
- `lib/bombadil/src/tree.rs`
- `lib/bombadil/src/trace/` (made generic over `<A, S>`)
- `lib/bombadil/src/styled.rs`

New in `bombadil`:
- `lib/bombadil/src/driver.rs` — `InterfaceDriver` trait + `DriverEvent`.

## Stage-by-stage work (one branch, jj revisions)

Use `jj` (or `nix run nixpkgs#jj` if not on PATH) to create a new rev per
stage. Each stage compiles and tests green before moving on.

### Stage 1 — Schema generalization

`bombadil-schema/src/schema.rs:70`: make `TraceEntry` generic over `<A, S>`:
the `action: Option<A>` and a new `state: S` payload (or keep the current
inlined fields and parametrize only `action` — decide after reading the
inspect consumer; the user said "generic on both" so go all the way and
nest browser-specific fields under `S`).

Move `BrowserAction` out of `bombadil-schema` into `bombadil-browser` (it
lives in the browser crate going forward). Inspect currently assumes browser
traces — that continues to work because `bombadil inspect` will keep using
`TraceEntry<BrowserAction, BrowserStateSummary>`. A future `bombadil
terminal inspect` will parametrize over the terminal types.

Update `bombadil-schema` types that are pure (Time, Snapshot, Violation,
Formula, EventuallyViolation, PropertyViolation, Point) to stay as-is.
`Resources` is a browser-perf-metrics struct — move to `bombadil-browser`.

### Stage 2 — Workspace split (mechanical move)

Create `lib/bombadil-browser/` with its own `Cargo.toml`. Move the files
listed above. Add it to the workspace members in `Cargo.toml`.

Fix imports in `bombadil-cli` and `integration-tests`. At this stage,
`bombadil-cli` re-imports browser symbols from `bombadil-browser` rather
than `bombadil`. No behavior change yet — `Runner` still hard-codes
`Browser` and `BrowserAction`; just lives in two crates.

### Stage 3 — `InterfaceDriver` trait + browser impl

Add `lib/bombadil/src/driver.rs` with the trait above.

In `bombadil-browser`, implement `InterfaceDriver` for the existing
`Browser` struct. The current `Browser::next_event` and `apply` map almost
1:1. `extract_snapshots` wraps the current `runner.rs:run_extractors` body
(it stays a CDP-based call, executed against the Chromium JS context).
`filter_actions` runs the `is_within_domain` check (`runner.rs:177`).

Genericize `Runner` and `RunStrategy` over `D: InterfaceDriver`. Replace
the direct `BrowserAction` / `BrowserState` references in `runner.rs:1-237`
with associated types. Move the JsAction-tree conversion into the driver
trait (or keep it in core if every driver is going to do the same
JSON→Action deserialization, which the worker already does generically via
`step::<A>`).

### Stage 4 — TS package reshape

All TypeScript stays in `lib/bombadil/src/specification/` and is embedded
via the existing `include_dir!("$CARGO_MANIFEST_DIR/src/specification")`
in `resolver.rs:10`. The browser and terminal Rust crates own zero TS;
they consume the same embedded bundle. Single TS package, single
embedding point, per-driver submodules.

Concrete file moves under `lib/bombadil/src/specification/`:
- New `browser.ts`: takes the current `State` interface from
  `index.ts:226-254`, takes the `Action` / `ActionGenerator` / `actions()`
  from `actions.ts:13-88`, and declares
  `export const runtime = new Runtime<BrowserState>()`.
- New `terminal.ts`: defines `TerminalState`, `TerminalAction`,
  `actions()` factory, and
  `export const runtime = new Runtime<TerminalState>()`.
- `index.ts` keeps the generic Formula API (`now`, `next`, `always`,
  `eventually`, `extract<S>`) and stops exporting the browser `State`
  interface or the browser `runtime` instance.
- `actions.ts` keeps the cross-driver pieces (`Tree<T>`, `Generator`,
  `from`, `strings`, etc.) and stops exporting the browser `Action` union.

`resolver.rs:147` already maps `@antithesishq/bombadil/<sub>` to
`<sub>.ts` in the embedded dir via `.with_added_extension("ts")`. New
submodules (`browser`, `terminal`) work without code changes — just new
files in the embedded directory.

The Rust-side `BombadilExports::from_object` (`specification/js.rs:340`)
grabs `runtime` from the bundle's namespace via the `__bombadilRequire`
machinery already in `verifier.rs:139-159`. Each spec imports a single
driver submodule, so a `require('@antithesishq/bombadil/browser')` (or
`/terminal`) returns the right `runtime` instance. The Rust loader needs
to know which submodule to require for a given run — pass it in alongside
the spec's `module_specifier` (e.g. as part of `Specification` or a
new `RuntimeModule` field).

### Stage 4.5 — Move browser TS under `browser/`

Breaking-change cleanup before adding a terminal driver. Everything
browser-specific in the TS package moves under a `browser/` directory
segment so generic vs driver-specific is unambiguous at a glance.

File moves under `lib/bombadil/src/specification/`:
- `browser.ts`              → `browser/index.ts`
- `defaults.ts`             → `browser/defaults.ts`
- `defaults/actions.ts`     → `browser/defaults/actions.ts`
- `defaults/properties.ts`  → `browser/defaults/properties.ts`
- (symmetrically) `terminal.ts` → `terminal/index.ts`

What stays at the root (generic, both drivers consume):
- `index.ts`     — Formula API (`now`, `next`, `always`, `eventually`, `not`)
- `actions.ts`   — `Tree<T>`, `ActionGenerator<A>`, `makeActions`, `makeWeighted`
- `random.ts`    — `from`, `strings`, `emails`, `integers`, `keycodes`, `randomRange`, `Generator`
- `internal.ts`  — `Runtime<S>`, `ExtractorCell<T, S>`, `JSON`, `Cell`
- `global.d.ts`

Resolver change (`specification/resolver.rs`): when
`@antithesishq/bombadil/<path>` is requested, try `<path>.ts` first,
then fall back to `<path>/index.ts`. Existing `with_added_extension`
logic stays as the file form; the directory form is the new fallback.
Same applies recursively for nested subpaths like
`@antithesishq/bombadil/browser/defaults`.

Import paths to update:
- `browser/defaults.ts` re-exports from
  `@antithesishq/bombadil/browser/defaults/properties` and
  `@antithesishq/bombadil/browser/defaults/actions`, and imports
  `weighted` from `@antithesishq/bombadil/browser`.
- `browser/defaults/actions.ts` and `.../properties.ts` import from
  `@antithesishq/bombadil/browser`.
- All integration-test fixtures and the `bombadil-inspect`
  test spec keep importing from `@antithesishq/bombadil/browser` and
  `@antithesishq/bombadil/browser/defaults` — same specifiers, different
  files behind them.
- `bombadil-cli` default `module_specifier` becomes
  `@antithesishq/bombadil/browser/defaults` (was
  `@antithesishq/bombadil/defaults`).
- `tsconfig.json` `paths` mapping gains the `browser/...` and
  `terminal/index` entries; old `defaults*` and root `browser/terminal`
  mappings are dropped.
- `lib/nix/npm-package.nix` exports map is rewritten for the new
  layout. The example `import` in the bundled README points at
  `@antithesishq/bombadil/browser` (already done) but the defaults
  re-export example needs updating to
  `@antithesishq/bombadil/browser/defaults`.

Bundler snapshot test will need to be re-accepted (the bundled tree
includes different module IDs).

### Stage 5 — Terminal driver (revised after libghostty Send surprise)

The naive "wrap Terminal + PtyProcess in a struct" approach doesn't
compile because libghostty's `Terminal` is not Send (its internal
`Box<dyn DeviceAttributesFn>` lacks a Send bound) and Boa's runtime
context is neither Send nor Sync. `InterfaceDriver: Send` requires the
driver struct itself to be Send, so the I/O resources have to live
behind a worker boundary — same pattern as `VerifierWorker`.

Target structure:

```
TerminalDriver (Send: just channels)
   │
   │ mpsc::Sender<TerminalCommand>          ─────────┐
   │ mpsc::Receiver<TerminalEvent>          ────────┐│
   │ ExtractorWorker handle (already Send) ────────┐││
   ▼                                                ▼▼▼
 (Runner-side, on the test thread)         (TerminalWorker thread)
                                            ┌─────────────────────┐
                                            │  Terminal<'static>  │
                                            │  PtyProcess         │
                                            │  PtyOutput          │
                                            │  Size               │
                                            └─────────────────────┘
                                            (the rendering Boa worker
                                             stays on its own thread,
                                             unchanged from current code)
```

Two worker threads in total per terminal run:
1. **TerminalWorker** — owns `Terminal` + PTY + size + last_action.
2. **ExtractorWorker** — already correct in current code; owns a
   private Boa context, runs extractors.

The `TerminalDriver` struct is a thin Send handle holding channel
endpoints to both workers. Build it on the runner side.

Message protocol with TerminalWorker (sketch):

```
enum TerminalCommand {
    Initiate { reply: oneshot::Sender<Result<()>> },
    Apply { action: TerminalAction, reply: oneshot::Sender<Result<()>> },
    AwaitState { reply: oneshot::Sender<TerminalEventResult> },
    Terminate { reply: oneshot::Sender<Result<()>> },
}

enum TerminalEventResult {
    StateChanged(TerminalState),
    Eof,
    Error(anyhow::Error),
}
```

Implementation details:

- TerminalWorker thread runs a `tokio::runtime::Builder::new_current_thread`
  inside; PTY I/O uses tokio; commands are received over a blocking
  channel and dispatched onto the worker's runtime. Or simpler: do all
  I/O synchronously inside the worker thread (`read` from
  portable-pty's reader, sleeps via `std::thread::sleep`) and reserve
  tokio for the runner side only. The current `PtyOutput` uses tokio
  mpsc, but we can drop that and use a `std::sync::mpsc` or just call
  `reader.read` directly inside the worker.
- `next_event` on the driver sends an `AwaitState` command and awaits
  the reply. The worker side drains its output buffer, quiesces (50 ms
  idle), builds a `TerminalState`, sends it back.
- `apply` sends an `Apply` command and awaits ack (so the action lands
  before the next `next_event`).
- `extract_snapshots` keeps current behavior: serialize the state JSON
  on the runner side, ship to ExtractorWorker, await snapshots.

Open question worth pausing on: do we want `next_event`'s
acknowledgement to be the same boundary as the action? An alternative
shape is for the worker to push StateChanged events into a channel
proactively (closer to how `Browser` works), and the runner's
`next_event` just `recv`s. That avoids round-trip latency but adds
backpressure handling. The simpler request/response shape is fine for
v1.

### Stage 5 — original notes (still applicable for the non-driver pieces)

In `bombadil-terminal`:
- `TerminalAction` (TypeText, PressKey, Scroll, Resize) with serde —
  already done.
- `TerminalState` (rendered grid summary, dimensions, scrollback,
  last action, timestamp) — already done.
- `extract_snapshots` via a private Boa worker — already done as
  `ExtractorWorker`. Keep.
- Replace the standalone `random_action` loop in
  `lib/bombadil-terminal/src/lib.rs` with a `Runner<TerminalDriver>`
  invocation that takes a spec module specifier — already done. Will
  need a small touch-up to call the new handle-based driver.

### Stage 6 — CLI

Extend `bombadil-cli` with a `terminal test` subcommand parallel to
`test`. Both call `Runner::new(...).run(strategy)` with the appropriate
driver. `inspect` keeps assuming browser traces for now (per user note).

### Stage 7 — Tests

Move browser integration tests to depend on `bombadil-browser`. Keep
fixtures where they are. Add a terminal smoke test that runs a trivial
spec (`always(() => grid.current.includes("ready"))`) against a fixture
PTY program.

## Verification

Implementation happens in an offline sandbox where the network and the
Rust toolchain may not be reachable. Build/test verification is the
user's responsibility after the sandbox commits land. The checklist they
should run:

- `cargo build --workspace --exclude bombadil-inspect` clean.
- `cargo clippy --workspace --exclude bombadil-inspect --fix --allow-dirty`
  clean.
- `cargo fmt --all` clean.
- `cargo test -p integration-tests` passes — browser path unchanged.
- New: `cargo test -p bombadil-terminal` runs the new terminal smoke
  spec end-to-end.
- Manual smoke for browser:
  `RUST_LOG=bombadil=debug cargo run -p bombadil-cli -- test https://example.com --headless`
  still works.
- Manual smoke for terminal: a new
  `cargo run -p bombadil-cli -- terminal test --spec ./spec.ts -- bash -i`
  exits cleanly when properties pass.

Inside the sandbox, each `jj` revision should at least be reviewable as
a coherent diff; reach out for guidance if a stage requires inspecting
build output to make a design choice.

## Open questions / risks

- **Boa instance count.** Stage 5 spins up a second Boa in the terminal
  driver. The verifier worker already runs a Boa on its own thread. Two
  Boa contexts is fine; they don't share state. The terminal driver's
  Boa is purely for `runtime.runExtractors`; property evaluation still
  happens in the verifier worker.
- **Bundle reuse.** We can bundle the spec once and pass the bundled
  source string to both the verifier worker and the driver's private
  Boa, instead of bundling twice. Cheap win.
- **`include_dir` boundary.** The embedded `JS_DIR` stays in `bombadil`
  core; both drivers share it. New per-driver TS submodules live as
  sibling files (`browser.ts`, `terminal.ts`) under the same
  `lib/bombadil/src/specification/` root. Stage 4 verifies oxc's
  resolver treats `@antithesishq/bombadil/terminal` correctly given the
  current `.with_added_extension("ts")` logic in `resolver.rs:147-164`.
- **Workspace dep cycle.** `bombadil-browser` and `bombadil-terminal`
  both depend on `bombadil`. `bombadil-cli` depends on all three. No
  cycle.
