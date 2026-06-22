# Contributing

## Developer environment

You can either use the Nix dev shell (recommended, provides everything pinned)
or install the toolchain yourself.

### With Nix

```bash
nix-shell lib/nix/shell.nix
# or, if you have direnv:
direnv allow .
```

### Without Nix

Install the toolchain by hand:

- **Rust** stable (latest), with the `wasm32-unknown-unknown` target.
  Install via [rustup](https://rustup.rs) and `rustup target add
  wasm32-unknown-unknown`.
- **Zig** 0.15.2 — required by `libghostty-vt-sys` to build the embedded
  ghostty terminal. Get it from <https://ziglang.org/download/>.
- **trunk** + **wasm-bindgen-cli** + **binaryen** — for building the
  `bombadil-inspect` WASM frontend that `bombadil-cli`'s build script bundles.
  `wasm-bindgen-cli` must match the `=X.Y.Z` pin in `Cargo.toml`.
- **clang**, **pkg-config**, **cmake**, **git** — native build deps for
  `bombadil-terminal`.
- **Chrome/Chromium** — for the integration tests that drive a real browser.

For release script:

- **Python 3** + **gh** + **basedpyright** + **black** — for the release
  scripts in `lib/release/`.

The CI workflow (`.github/workflows/ci.yml`) is the source of truth for
the exact versions and steps; reproduce it locally if you're matching its
behavior.

### Documentation shell

Documentation building requires Pandoc and TeXLive, kept out of the default
shell to keep it lighter. To work on the manual in `docs/manual/`:

```bash
cd docs/manual
direnv allow  # or nix-shell
make dev      # or make html, make pdf, make epub, etc for one-off builds.
```

## Debugging

See debug logs:

```bash
RUST_LOG=bombadil=debug cargo run -- browser test https://example.com --headless
```

### Bombadil Inspect

Inspect a trace file with Bombadil Inspect:

```bash
cargo run -- inspect /path/to/trace
```

To work on the Inspect frontend:

```bash
cd lib/bombadil-inspect
trunk serve
```

This only runs the frontend. Run the backend using the `inspect` command in a
separate tab.

## Development

### Integration tests

```bash
cargo test -p bombadil-browser-integration-tests
```

## Releasing

Run the release script from the repo root (inside the default dev shell, or
with `python3`/`gh` on your PATH):

```bash
python3 lib/release/main.py
```

The script guides you through all steps interactively: version selection,
branch creation, version bump, changelog update, PR creation, tagging, and
publishing the GitHub release.
