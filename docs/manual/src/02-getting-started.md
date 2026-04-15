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
<summary>npm</summary>

Install as a development dependency in your project:

```bash
npm install --save-dev @antithesishq/bombadil
```

Add a script to your `package.json` to run Bombadil:

```json
{
  "scripts": {
    "test": "bombadil test https://your-app.example.com"
  }
}
```

Then run it with `npm test`. This also provides TypeScript type definitions for
writing specifications.

</details>
<details name="install">
<summary>macOS</summary>

Download the `bombadil` binary using `curl` (or `wget`) and make it executable:

```bash
curl -L -o bombadil https://github.com/antithesishq/bombadil/releases/download/v%version%/bombadil-aarch64-darwin
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
curl -L -o bombadil https://github.com/antithesishq/bombadil/releases/download/v%version%/bombadil-x86_64-linux
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

* Docker images
* a GitHub Action, ready to be used in your CI configuration

If you want to compile from source, see [Contributing](https://github.com/antithesishq/bombadil/tree/main/docs/development/contributing.md).

## TypeScript support

When writing specifications in TypeScript, you'll want the types available.
If you installed Bombadil via npm, you already have them — skip ahead to
[Your first test](#your-first-test).

Otherwise, install the package with your package manager of choice:


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
option you will get results to inspect afterwards.

Launch the *Bombadil Inspect* tool to see what happened in the test you
just ran:

```bash
bombadil inspect my-test
```

This will open a web application in your browser, which has some features to highlight:

* This interface is focused on *state transitions*, i.e. the state before and
  after each action.
* On the left is the actions list, which you can use to navigate the state
  transitions by clicking the actions. 
* In the bottom you'll see the timeline, which you can scrub (click and drag)
  with your mouse. The timeline also shows the currently selected state
  transition.
* If there were any violations found in the test, they'll be shown as
  exclamation mark icons in the timeline.

No violations? That's fine, Wikipedia is pretty solid! This confirms that
Bombadil runs and produces results.
