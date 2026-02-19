# Getting Started

## Installation

The most straightforward way for you to get started is downloading the
executable for your platform:

<div class="accordion">
<details name="install">
<summary>macOS</summary>

Download the `bombadil` binary using `curl` (or `wget`) and make it executable:

```bash
$ curl -L -o bombadil https://github.com/antithesishq/bombadil/releases/v%version%/download/bombadil-aarch64-darwin
$ chmod +x bombadil
```

Put the binary somewhere on your `PATH`, like in `~/.local/bin` if that is
configured.

```bash
$ mv ./bombadil ~/.local/bin/bombadil
```

You should now be able to run it:

```bash
$ bombadil --version
```

::: {.callout .callout-warning}
Do not download the executable with your web browser. It will be blocked by GateKeeper.
:::

</details>
<details name="install">
<summary>Linux</summary>

Download the `bombadil` binary and make it executable:

```bash
$ curl -L -o bombadil https://github.com/antithesishq/bombadil/releases/v%version%/download/bombadil-x86_64-linux
$ chmod +x bombadil
```


Put the binary somewhere on your `PATH`, like in `~/.local/bin` if that is
configured.

```bash
$ mv ./bombadil ~/.local/bin/bombadil
```

You should now be able to run it:

```bash
$ bombadil --version
```

</details>
<details name="install">
<summary>Nix (flake)</summary>

```bash
$ nix run github:antithesishq/bombadil
```

</details>
</div>

Not yet available, but coming soon:

* Docker images
* a GitHub Action, ready to be used in your CI configuration

If you want to compile from source, see [Contributing](https://github.com/antithesishq/bombadil/tree/main/docs/contributing.md).

## TypeScript Support

When writing specifications in TypeScript, you'll want the types available.
Get them from [NPM](https://www.npmjs.com/package/@antithesishq/bombadil)
with your package manager of choice:


<div class="accordion">
<details name="typescript">
<summary>npm</summary>
```bash
$ npm install --save-dev @antithesishq/bombadil
```
</details>
<details name="typescript">
<summary>Yarn</summary>
```bash
$ yarn add --dev @antithesishq/bombadil
```
</details>
<details name="typescript">
<summary>Bun</summary>
```bash
$ bun add --development @antithesishq/bombadil
```
</details>
</div>

Or use the files provided in [the 
release package](https://github.com/antithesishq/bombadil/releases/v%version%).

## Quick Start

TODO: Quick start guide

## Your First Test

TODO: Step-by-step first test
