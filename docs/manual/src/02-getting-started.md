# Getting Started

## Installation

The most straightforward way for you to get start is downloading [the latest
executable](https://github.com/antithesishq/bombadil/releases/latest) for your
platform:

<details>
<summary>Executable (macOS)</summary>
```bash
$ curl -L -o bombadil https://github.com/antithesishq/bombadil/releases/latest/download/bombadil-aarch64-darwin
$ chmod +x bombadil
$ ./bombadil --version
```
</details>

<details>
<summary>Executable (Linux)</summary>
```bash
$ curl -L -o bombadil https://github.com/antithesishq/bombadil/releases/latest/download/bombadil-x86_64-linux
$ chmod +x bombadil
$ ./bombadil --version
```
</details>

If you're a Nix and flakes user, you can run it with:

```
$ nix run github:antithesishq/bombadil
```

Not yet available, but coming soon:

* Docker images
* a GitHub Action, ready to be used in your CI configuration

If you want to compile from source, see [Contributing](docs/contributing.md).

### TypeScript Support

When writing specifications in TypeScript, you'll want the types available.
Get them from [NPM](https://www.npmjs.com/package/@antithesishq/bombadil)
with your package manager of choice:

```bash
$ npm install @antithesishq/bombadil
```

Or use the files provided in the [the 
release package](https://github.com/antithesishq/bombadil/releases/latest).

## Quick Start

TODO: Quick start guide

## Your First Test

TODO: Step-by-step first test
