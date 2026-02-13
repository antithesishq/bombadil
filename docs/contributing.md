# Contributing

## Developer Environment

The blessed setup is using the Nix flake to get a shell.

```bash
nix develop
# or if you have direnv:
direnv allow .
```

## Debugging

See debug logs:

```bash
RUST_LOG=bombadil=debug cargo run -- test https://example.com --headless
```

There's also [VSCode launch configs](development/launch.json) for debugging
with codelldb. These have only been tested from `nvim-dap`, though. Put that
in `.vscode/launch.json` and modify at will.

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
cargo test --test integration_tests
```

### Changing dependencies

After any changes to dependencies in Cargo.toml:

```bash
crate2nix generate -o nix/Cargo.nix
```

## Releasing

1. Make sure you're on branch `main` and in a clean state
1. Create a new branch `release/x.y.z` (with the actual version)
1. Bump the version in `Cargo.toml`
1. `cargo check` (this regenerates the `Cargo.lock` file)
1. Run:

   ```
   export VERSION_PREV=$(git tag --sort=-v:refname -l "v*" | sed -n '1p')
   export VERSION_NEW=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
   ```

1. Run:

   ```
   (echo "## ${VERSION_NEW}" && echo "" && git log v${VERSION_PREV}..HEAD --oneline | sed 's/^[a-z0-9]* /* /' && echo "") \
       | cat - CHANGELOG.md > tmp.md && mv tmp.md CHANGELOG.md
   ```

   Open up `CHANGELOG.md` and rewrite the commit log into something meaningful.

1. `git commit -m "release v${VERSION_NEW}"`
1. Push to GitHub and create a pull request. Once reviewed and merged to continue with:
1. `git fetch`
1. `git tag -am "v${VERSION_NEW}" <SQUASH COMMIT FROM PULL REQUEST>`
1. `git push origin "v${VERSION_NEW}"`

The release workflow will then build binaries, publish the types package to
NPM, and create a **draft** GitHub release. Go to the GitHub releases page,
review the draft, and publish it.
