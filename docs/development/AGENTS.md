# AGENTS.md

This file provides guidance to coding agents when working with code in this repository.

## What is Bombadil?

Bombadil is a property-based testing tool for web UIs built by Antithesis. Users write specifications as TypeScript modules exporting LTL (Linear Temporal Logic) properties. Bombadil autonomously explores the web app via a browser and checks those properties at each state, reporting violations. This is fuzzing/property-based testing for web applications, not fixed test cases.

## Build & Development

Agents should be run inside a Nix development shell so all tools are available. If not, prefix commands with `nix --extra-experimental-features 'nix-command flakes' develop --command`.

There are two development shells:

- **default**: For general development (Rust, TypeScript, testing). Does not include documentation tools.
- **manual**: For building the manual (includes Pandoc, TeXLive, fonts). Use this in `docs/manual/`.

The `docs/manual/.envrc` file automatically loads the `manual` shell when you `cd` into that directory (requires direnv).

**Build:** `cargo build`

**Integration tests:** `cargo test -p integration-tests` (limited to 4 concurrent tests; 120s timeout each)

**Debug logging:** `RUST_LOG=bombadil=debug cargo run -p bombadil-cli -- test https://example.com --headless`

## Code Quality

**IMPORTANT:** After making any changes to Rust code, ALWAYS run:

```bash
cargo build --workspace --exclude bombadil-inspect
cargo clippy --workspace --exclude bombadil-inspect --fix --allow-dirty
cargo fmt --all
```

This ensures code follows project conventions and passes CI checks.

## Architecture

Rust backend + TypeScript specification layer, connected via the Boa JavaScript engine at runtime.

### Workspace structure

The project is a Cargo workspace with crates under `lib/`:

- **`lib/bombadil/`** - Core library (see modules below)
- **`lib/bombadil-ltl/`** - Generic finite LTL syntax and evaluator (standalone, no Bombadil-specific dependencies)
- **`lib/bombadil-browser-keys/`** - Browser key name definitions
- **`lib/bombadil-schema/`** - JSON schema definitions
- **`lib/bombadil-cli/`** - CLI binary (test commands, inspect server)
- **`lib/bombadil-terminal/`** - Terminal UI
- **`lib/bombadil-inspect/`** - Yew WASM frontend for Bombadil Inspect
- **`lib/integration-tests/`** - Integration tests with browser fixtures

Non-workspace directories: `lib/nix/` (Nix build infrastructure), `lib/release/`, `lib/experiments/`.

### Core library modules (`lib/bombadil/`)

- **runner** - Test orchestration loop. Drives the browser, invokes the verifier, publishes `RunEvent`s.
- **browser** - Chromium control via CDP. Defines `BrowserState` snapshots and `BrowserAction`s.
- **specification** - Split between Rust and TypeScript. Rust side loads spec files and runs the Boa JS engine. TypeScript side provides the user-facing API for defining properties, action generators, and extractors.
- **instrumentation** - JS code coverage via edge maps using Oxc.
- **tree** - Weighted tree for random action selection.
- **trace** - JSONL trace writer with screenshots.
- **geometry** - Geometric primitives (rectangles, points) for element layout.
- **url** - Domain boundary enforcement.

### Rust-TypeScript bridge

1. TypeScript source files are embedded directly into the binary via `include_dir`.
2. At bundle time, the Rust bundler uses oxc to strip TypeScript types and transform ESM to CommonJS.
3. At runtime, Boa engine loads the bundled JS modules.
4. Rust exposes native functions (e.g., `__bombadil_random_bytes()`) to the JS environment.
5. State snapshots are passed as JSON between layers.

### Async patterns

Heavy use of Tokio: async/await, broadcast channels for events, oneshot for synchronization, message-passing channels for cross-thread verifier communication.

## Formatting

- Rust: 80-char max width, 4-space indentation, no hard tabs (`.rustfmt.toml`)
- TypeScript/JS: formatted with biome (available in dev shell)

## Naming Conventions

Use clear, descriptive names without needless abbreviations:
- `statement` not `stmt`
- `expression` not `expr`
- `string` not `str` (except for the Rust type `&str`)
- `declaration` not `decl`
- `specifier` not `spec`
- `identifier` not `ident`

Exception: `ctx` is acceptable for `TraverseCtx` parameters since it's used pervasively in oxc traversal code. Never use `ctx` outside `oxc` code.

## Comments

Do NOT add verbose comments that restate what the code does. Only add comments where:
- The logic is genuinely non-obvious or has a subtle reason
- You need to explain "why" something is done, not "what" is being done
- There's a workaround for a library limitation or bug

Bad (don't do this):
```rust
// Create a require call
let require_call = ctx.ast.expression_call(...);

// Get module.exports
let module_exports = ctx.ast.member_expression_static(...);
```

Good (acceptable):
```rust
// Use Object.assign instead of for-in loop to avoid stack overflow in oxc traverse
let object_assign_call = ...;
```

## Testing

Integration tests are in `lib/integration-tests/tests/`. Each test scenario has an HTML fixture directory (e.g., `tests/links/`, `tests/console-error/`). Tests spawn local web servers (axum) and run Bombadil against them. Snapshot tests use `insta`. Property tests use `proptest`.
