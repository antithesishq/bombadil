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

1. Bump the version in `Cargo.toml`
2. Commit: `git commit -am "v0.x.x"`
3. Tag: `git tag v0.x.x`
4. Push: `git push origin main --tags`

The release workflow will build binaries, create a GitHub release, and publish
the types package to npm.
