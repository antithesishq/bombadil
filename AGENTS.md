# AGENTS.md

This file provides guidance to coding agents when working with code in this repository.

## What is Bombadil?

Bombadil is a property-based testing tool for web UIs built by Antithesis. Users write specifications as TypeScript modules exporting LTL (Linear Temporal Logic) properties. Bombadil autonomously explores the web app via a browser and checks those properties at each state, reporting violations. This is fuzzing/property-based testing for web applications, not fixed test cases.

## Build & Development

All commands (cargo, esbuild, etc.) must be run via `nix develop --command`. Do not open an interactive shell. Wrap every command invocation like this:

```bash
nix --extra-experimental-features 'nix-command flakes' develop --command <cmd>
```

The `--extra-experimental-features 'nix-command flakes'` flag is required for all `nix` invocations.

**Build:** `nix --extra-experimental-features 'nix-command flakes' develop --command cargo build` (the build script in `src/build.rs` runs esbuild to compile `src/specification/**/*.ts` into `target/specification/`)

**Integration tests:** `nix --extra-experimental-features 'nix-command flakes' develop --command cargo test --test integration_tests` (limited to 2 concurrent tests; 120s timeout each)

**All checks via Nix:** `nix --extra-experimental-features 'nix-command flakes' flake check .` (runs clippy, fmt, tests)

**Debug logging:** `nix --extra-experimental-features 'nix-command flakes' develop --command bash -c 'RUST_LOG=bombadil=debug cargo run -- test https://example.com --headless'`

## Architecture

Rust backend + TypeScript specification layer, connected via the Boa JavaScript engine at runtime.

### Core modules (`src/lib.rs`)

- **runner** (`src/runner.rs`) - Test orchestration loop. Drives the browser, invokes the verifier, publishes `RunEvent`s (state transitions, violations). The main entrypoint for a test run.
- **browser** (`src/browser/`) - Chromium control via CDP (`chromiumoxide`). `state.rs` defines `BrowserState` snapshots (URL, title, console, exceptions, DOM). `actions.rs` defines `BrowserAction` (Click, TypeText, PressKey, Scroll, navigation). `instrumentation.rs` injects coverage tracking JS.
- **specification** - Split between Rust and TypeScript:
  - `verifier.rs` - Loads spec files, runs Boa JS engine, evaluates properties, manages extractors.
  - `worker.rs` - Runs verifier in a separate OS thread with message passing.
  - `ltl.rs` - LTL formula evaluation engine (always, eventually, next, implies, etc.) with violation tracking.
  - TypeScript files (`index.ts`, `actions.ts`, `defaults.ts`, `internal.ts`) - User-facing API for defining properties, action generators, and extractors. Compiled to ESM by esbuild at build time, embedded via `include_dir`.
- **tree** (`src/tree.rs`) - Weighted tree for random action selection. `pick()` traverses using RNG.
- **instrumentation** (`src/instrumentation/`) - JS code coverage via edge maps using Oxc. `html.rs` instruments inline scripts.
- **trace** (`src/trace/`) - JSONL trace writer with screenshots.
- **url** (`src/url.rs`) - Domain boundary enforcement.

### Rust-TypeScript bridge

1. `src/build.rs` compiles `.ts` files to `.js` ESM modules via esbuild at build time.
2. At runtime, Boa engine loads the bundled JS modules.
3. Rust exposes native functions (e.g., `__bombadil_random_bytes()`) to the JS environment.
4. State snapshots are passed as JSON between layers.

### Async patterns

Heavy use of Tokio: async/await, broadcast channels for events, oneshot for synchronization, message-passing channels for cross-thread verifier communication.

## Formatting

- Rust: 80-char max width, 4-space indentation, no hard tabs (`.rustfmt.toml`)
- TypeScript/JS: formatted with biome (available in dev shell)

## Testing

Integration tests are in `tests/`. Each test scenario has an HTML fixture directory (e.g., `tests/links/`, `tests/console-error/`). Tests spawn local web servers (axum) and run Bombadil against them. Snapshot tests use `insta`. Property tests use `proptest`.
