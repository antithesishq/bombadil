# Contributing

## Developer environment

You can either use the Nix dev shell (provides everything pinned) or install
the toolchain yourself.

### With Nix

```bash
nix develop
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
  `wasm-bindgen-cli` must match the `=X.Y.Z` pin in `Cargo.toml`; mismatches
  produce broken bindings.
- **clang**, **pkg-config**, **cmake**, **git** — native build deps for
  `bombadil-terminal`.
- **chromium** — for the integration tests that drive a real browser.
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
direnv allow  # loads the 'manual' shell automatically
make html     # or make pdf, make epub, etc.
```

Or run directly:

```bash
nix develop '.#manual' --command make -C docs/manual pdf
```

## Workspace structure

The project is a Cargo workspace under `lib/`. See `AGENTS.md` for a
crate-by-crate breakdown. Build individual crates with `-p`:

```bash
cargo build -p bombadil       # Core library only
cargo build -p bombadil-cli   # CLI binary (includes library + inspect WASM)
```

## Debugging

See debug logs:

```bash
RUST_LOG=bombadil=debug cargo run -- test https://example.com --headless
```

There's also [VSCode launch configs](development/launch.json) for debugging
with codelldb. These have only been tested from `nvim-dap`, though. Put that
in `.vscode/launch.json` and modify at will.

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

## Running in podman

On a Linux host, build the binary with your default toolchain and bake the
image. The `--target x86_64-unknown-linux-musl` step is only needed if you
want to mirror CI's fully-static release binary.

```bash
cargo build --release -p bombadil-cli
mkdir -p artifact && cp target/release/bombadil artifact/
docker build -t bombadil_docker:latest .  # use the Dockerfile CI generates
```

Run it:

```bash
podman run -ti localhost/bombadil_docker:latest <SOME_URL>
```

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
