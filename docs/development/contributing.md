# Contributing

## Developer Environment

The blessed setup is using the Nix flake to get a shell.

```bash
nix develop
# or if you have direnv:
direnv allow .
```

### Documentation Shell

Documentation building requires a separate shell with Pandoc and TeXLive. This keeps the default development environment lighter.

To work on the manual in `docs/manual/`:

```bash
cd docs/manual
direnv allow  # loads the 'manual' shell automatically
make html     # or make pdf, make epub, etc.
```

Or run commands directly:

```bash
nix develop '.#manual' --command make -C docs/manual pdf
```

## Workspace Structure

The project is organized as a Cargo workspace under `lib/`:

```
lib/
├── bombadil/           # Core library (browser, runner, trace, specification, etc.)
├── bombadil-cli/       # CLI binary (test commands, debug server)
├── bombadil-debug-ui/  # Yew frontend (WASM debug UI)
├── integration-tests/  # Integration tests with browser fixtures
└── nix/                # Nix build infrastructure
```

Build specific crates with `-p`:

```bash
cargo build -p bombadil       # Core library only
cargo build -p bombadil-cli   # CLI binary (includes library)
```

## Debugging

See debug logs:

```bash
RUST_LOG=bombadil=debug cargo run -p bombadil-cli -- test https://example.com --headless
```

There's also [VSCode launch configs](development/launch.json) for debugging
with codelldb. These have only been tested from `nvim-dap`, though. Put that
in `.vscode/launch.json` and modify at will.

### Debug UI

Inspect a trace file with the debug UI:

```bash
cargo run -p bombadil-cli -- debug /path/to/trace
```

To work on the debug UI frontend:

```bash
cd lib/bombadil-debug-ui
trunk build --dist ../../target/debug-ui
```

The CLI build script automatically rebuilds the debug UI when trunk is
available. If trunk is not installed, a placeholder page is served instead.

## Running in podman

Build and tag the image:

```bash
nix build ".#docker" \
    && podman load < result \
    && podman tag localhost/bombadil_docker:$(nix eval --raw '.#packages.x86_64-linux.docker.imageTag') localhost/bombadil_docker:latest
```

Run it:

```bash
podman run -ti localhost/bombadil_docker:latest <SOME_URL>
```

## Development

### Integration tests

```bash
cargo test -p integration-tests
```

## Releasing

1. Make sure you're on branch `main` and in a clean state
1. Create a new branch `release/x.y.z` (with the actual version)
1. Bump the version in the root `Cargo.toml` under `[workspace.package]`
1. `cargo check` (this regenerates the `Cargo.lock` file)
1. Run:

   ```
   export VERSION_PREV=$(git tag --sort=-v:refname -l "v*" | sed -n '1p' | sed 's/v//')
   export VERSION_NEW=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
   ```

1. Run:

   ```
   tail -n +2 CHANGELOG.md > tmp.md
   rm -f CHANGELOG.md
   (echo "# The Bombadil Changelog" && echo "" && echo "## ${VERSION_NEW}" && echo "" && git log v${VERSION_PREV}..HEAD --oneline | sed 's/^[a-z0-9]* /* /' && echo "") \
       | cat - tmp.md > CHANGELOG.md && rm -f tmp.md
   ```

   Open up `CHANGELOG.md` and rewrite the commit log into something meaningful.

1. `git add .`
1. `git commit -m "release v${VERSION_NEW}"`
1. Push to GitHub and create a pull request.

   Review the changes and let the checks pass. Then merge the PR and continue:
1. `git fetch`
1. `git tag -a "v${VERSION_NEW}" -m "v${VERSION_NEW}" <SQUASH COMMIT FROM PULL REQUEST>`
1. `git push origin "v${VERSION_NEW}"`

The release workflow will then build binaries, publish the types package to
NPM, and create a **draft** GitHub release. Go to the GitHub releases page,
review the draft, and publish it.
