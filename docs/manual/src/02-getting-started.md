# Getting started

Bombadil runs on your development machine if you're on macOS or Linux. You can
use it to validate changes to [TypeScript
specifications](#properties), and to run short
tests while working on your system. Then you'll have something like GitHub
Actions to run longer tests on your main branch or in nightlies.

## Installation

The most straightforward way for you to get started is downloading the
executable for your platform:

<div class="accordion">
<details name="install">
<summary>macOS</summary>

Download the `bombadil` binary using `curl` (or `wget`) and make it executable:

```bash
curl -L -o bombadil https://github.com/antithesishq/bombadil/releases/v%version%/download/bombadil-aarch64-darwin
chmod +x bombadil
```

Put the binary somewhere on your `PATH`, like in `~/.local/bin` if that is
configured.

```bash
mv ./bombadil ~/.local/bin/bombadil
```

You should now be able to run it:

```bash
bombadil --version
```

::: {.callout .callout-warning}
Do not download the executable with your web browser. It will be blocked by GateKeeper.
:::

</details>
<details name="install">
<summary>Linux</summary>

Download the `bombadil` binary and make it executable:

```bash
curl -L -o bombadil https://github.com/antithesishq/bombadil/releases/v%version%/download/bombadil-x86_64-linux
chmod +x bombadil
```


Put the binary somewhere on your `PATH`, like in `~/.local/bin` if that is
configured.

```bash
mv ./bombadil ~/.local/bin/bombadil
```

You should now be able to run it:

```bash
bombadil --version
```

</details>
<details name="install">
<summary>Nix (flake)</summary>

```bash
nix run github:antithesishq/bombadil
```

</details>
</div>

Not yet available, but coming soon:

* executables bundled in NPM package (i.e. `npx @antithesishq/bombadil`)
* Docker images
* a GitHub Action, ready to be used in your CI configuration

If you want to compile from source, see [Contributing](https://github.com/antithesishq/bombadil/tree/main/docs/contributing.md).

## TypeScript support

When writing specifications in TypeScript, you'll want the types available.
Get them from [NPM](https://www.npmjs.com/package/@antithesishq/bombadil)
with your package manager of choice:


<div class="accordion">
<details name="typescript">
<summary>npm</summary>
```bash
npm install --save-dev @antithesishq/bombadil
```
</details>
<details name="typescript">
<summary>Yarn</summary>
```bash
yarn add --dev @antithesishq/bombadil
```
</details>
<details name="typescript">
<summary>Bun</summary>
```bash
bun add --development @antithesishq/bombadil
```
</details>
</div>

Or use the files provided in [the 
release package](https://github.com/antithesishq/bombadil/releases/v%version%).

## Your first test

With the CLI installed, let's run a test just to see that things are working:

```bash
bombadil test https://en.wikipedia.org --output-path my-test
```

This will run until you shut it down using <kbd>CTRL</kbd>+<kbd>C</kbd>. Any
property violations will be logged as errors, and with the `--output-path`
option you get a JSONL file to inspect afterwards.

Find the URLs with violations (assuming you have `jq` installed):

```bash
jq -r 'select(.violations != []) | .url' my-test/trace.jsonl
```

Nothing? That's fine, Wikipedia is pretty solid! This confirms that
Bombadil runs and produces results.


::: {.callout .callout-note}
Bombadil doesn't yet produce a human-readable test report, so this
requires some `jq` trickery. Stay tuned, better UIs are on their way! 
:::
