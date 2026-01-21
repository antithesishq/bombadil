<h1 align="center">Bombadil</h1>
<p align="center">Advanced testing of any web or Electron app.</em></p>
<p align="center"><img alt="Tom Bombadil" src="tom.png" width=360 /></p>

## Usage

### Running tests

```bash
cargo run -- test https://example.com
```

Run headless:

```bash
cargo run -- test https://example.com --headless
```

See debug logs:

```bash
RUST_LOG=bombadil=debug cargo run -- test https://example.com --headless
```

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
