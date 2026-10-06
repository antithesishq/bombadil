# Contributing

Here are some resources to check out of first:

* [Bombadil Generative AI Policy](gen-ai-policy.md)

## Developer environment

The blessed setup is using the Nix flake to get a shell.

```bash
nix develop
# or if you have direnv:
direnv allow .
```

### Documentation shell

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

## Workspace structure

The project is organized as a Cargo workspace under `lib/`:

```
lib/
├── bombadil/           
├── bombadil-cli/       
├── bombadil-inspect/
├── bombadil-browser-integration-tests/  
├── ...
└── nix/                
```

Most of these directories should be creates, but can be other stuff, like
`lib/nix`.

Build specific crates with `-p`:

```bash
cargo build -p bombadil       # Core library only
cargo build -p bombadil-cli   # CLI binary (includes library)
```

## Debugging

See debug logs:

```bash
RUST_LOG=bombadil=debug cargo run -- browser test https://example.com --headless
```

There's also [VSCode launch configs](development/launch.json) for debugging
with codelldb. These have only been tested from `nvim-dap`, though. Put that
in `.vscode/launch.json` and modify at will.

### Bombadil Inspect

Inspect a trace file with Bombadil Inspect:

```bash
cargo run -- browser inspect /path/to/trace
```

To work on the Inspect frontend:

```bash
cd lib/bombadil-inspect
trunk serve
```

This only runs the frontend. Run the backend using the `inspect` command in a
separate tab.

## Running in Docker

Build and tag the image:

```bash
nix build ".#docker" && docker load < result
```

This will print something like:

```
Loaded image: antithesishq/bombadil:0.7.7
```

Run it:

```bash
docker run -ti antithesishq/bombadil:0.7.7 browser test --headless --no-sandbox https://en.wikipedia.org 
```

## Development

### Checks

These are useful to run locally before pushing and relying on CI:

```bash
cargo build --workspace
cargo clippy --fix --allow-dirty
cargo fmt --all
cargo test
```

Or run them with Nix:

```bash
nix flake check .
```

## Releasing

Run the release script from the repo root (in the default Nix shell):

```bash
release
```

The script guides you through all steps interactively: version selection,
branch creation, version bump, changelog update, PR creation, tagging, and
publishing the GitHub release.
