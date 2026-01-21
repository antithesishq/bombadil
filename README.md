# antithesis_browser

Generative browser testing for local use, CI, and Antithesis.

<img alt="You were never tested like I was" src="https://i.imgflip.com/ahxk0j.jpg" width=400 />

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
RUST_LOG=antithesis_browser=debug cargo run -- test https://example.com --headless
```

## Running in podman

Build and tag the image:

```bash
nix build ".#docker" \
    && podman load < result \
    && podman tag localhost/antithesis_browser_docker:$(nix eval --raw '.#packages.x86_64-linux.docker.imageTag') localhost/antithesis_browser_docker:latest
```

Run it:

```bash
podman run -ti localhost/antithesis_browser_docker:latest <SOME_URL>
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


## JS Instrumentation Proxy

First, start the proxy:

```bash
cargo run -- proxy --port=9000
```

Then run:

```bash
chromium --incognito --temp-profile --proxy-server="http://127.0.0.1:9000" --proxy-bypass-list="<-loopback>"
```
